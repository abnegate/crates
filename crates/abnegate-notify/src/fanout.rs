//! Delivering one notification to every channel at once.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::task::{Id, JoinSet};
use tokio::time::timeout;

use crate::delivery::Delivery;
use crate::error::NotifyError;
use crate::notification::Notification;
use crate::notifier::Notifier;
use crate::report::Report;

/// The default a channel gets before it is abandoned.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

/// Delivers a notification to every registered channel concurrently.
///
/// Each channel runs as its own task under its own timeout, so a webhook that
/// hangs, fails or panics costs exactly one entry in the [`Report`] and
/// nothing else. Nothing here returns a `Result`: a fan-out has no single
/// outcome, only one per channel.
pub struct Fanout {
    notifiers: Vec<Arc<dyn Notifier>>,
    timeout: Duration,
}

impl Default for Fanout {
    fn default() -> Self {
        Self::new()
    }
}

impl Fanout {
    pub fn new() -> Self {
        Self {
            notifiers: Vec::new(),
            timeout: DEFAULT_TIMEOUT,
        }
    }

    /// The budget for any channel that does not set its own.
    #[must_use]
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    #[must_use]
    pub fn with(mut self, notifier: impl Notifier) -> Self {
        self.register(Arc::new(notifier));
        self
    }

    pub fn register(&mut self, notifier: Arc<dyn Notifier>) {
        self.notifiers.push(notifier);
    }

    pub fn len(&self) -> usize {
        self.notifiers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.notifiers.is_empty()
    }

    /// Deliver to every channel, and report what each one did.
    ///
    /// With nothing registered this is an empty report rather than an error:
    /// a caller that has configured no channels has not failed at anything.
    pub async fn deliver(&self, notification: &Notification) -> Report {
        if self.notifiers.is_empty() {
            return Report::default();
        }

        let shared = Arc::new(notification.clone());
        let mut tasks = JoinSet::new();
        let mut indices: HashMap<Id, usize> = HashMap::with_capacity(self.notifiers.len());

        for (index, notifier) in self.notifiers.iter().enumerate() {
            let notifier = Arc::clone(notifier);
            let notification = Arc::clone(&shared);
            let budget = notifier.timeout().unwrap_or(self.timeout);

            let handle = tasks.spawn(async move {
                let outcome = match timeout(budget, notifier.deliver(&notification)).await {
                    Ok(outcome) => outcome,
                    Err(_) => Err(NotifyError::Timeout { after: budget }),
                };
                (index, describe(notifier.as_ref(), outcome))
            });
            indices.insert(handle.id(), index);
        }

        let mut collected: Vec<(usize, Delivery)> = Vec::with_capacity(self.notifiers.len());
        while let Some(joined) = tasks.join_next().await {
            match joined {
                Ok(entry) => collected.push(entry),
                Err(error) => {
                    let Some(&index) = indices.get(&error.id()) else {
                        continue;
                    };
                    let notifier = self.notifiers[index].as_ref();
                    tracing::error!(
                        channel = %notifier.channel(),
                        "Notification channel panicked"
                    );
                    collected.push((index, describe(notifier, Err(NotifyError::Panicked))));
                }
            }
        }

        collected.sort_by_key(|(index, _)| *index);
        let deliveries: Vec<Delivery> = collected.into_iter().map(|(_, one)| one).collect();

        for failure in deliveries.iter().filter(|one| !one.is_delivered()) {
            if let Some(error) = failure.error() {
                tracing::warn!(
                    channel = %failure.channel(),
                    name = failure.name(),
                    %error,
                    "Notification delivery failed"
                );
            }
        }

        Report::new(deliveries)
    }
}

fn describe(notifier: &dyn Notifier, outcome: Result<(), NotifyError>) -> Delivery {
    let channel = notifier.channel();
    let name = notifier
        .name()
        .unwrap_or_else(|| channel.as_str())
        .to_string();
    Delivery::new(channel, name, outcome)
}

/// The real backends driven through the fan-out.
///
/// The stub-driven tests in `tests/fanout.rs` cover the dispatch contract and
/// the backends' own tests cover their payloads; these two check that the
/// assembled thing works, which neither of those would catch on its own.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::{Discord, Slack};
    use crate::channel::Channel;
    use crate::severity::Severity;
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn real_backends_deliver_together() {
        let slack_server = MockServer::start().await;
        let discord_server = MockServer::start().await;

        for (server, status) in [(&slack_server, 200), (&discord_server, 204)] {
            Mock::given(method("POST"))
                .respond_with(ResponseTemplate::new(status))
                .expect(1)
                .mount(server)
                .await;
        }

        let report = Fanout::new()
            .with(Slack::at_test_server(&slack_server.uri()))
            .with(Discord::at_test_server(&discord_server.uri()))
            .deliver(&Notification::new("Build failed", "3 tests failed").severity(Severity::Error))
            .await;

        assert!(report.all_delivered(), "{report:?}");
        assert_eq!(report.len(), 2);
    }

    #[tokio::test]
    async fn a_broken_webhook_does_not_stop_the_other_channel() {
        let slack_server = MockServer::start().await;
        let discord_server = MockServer::start().await;

        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&slack_server)
            .await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(404).set_body_string("Unknown Webhook"))
            .mount(&discord_server)
            .await;

        let report = Fanout::new()
            .with(Slack::at_test_server(&slack_server.uri()))
            .with(Discord::at_test_server(&discord_server.uri()))
            .deliver(&Notification::new("Build failed", "3 tests failed"))
            .await;

        assert_eq!(report.delivered().count(), 1);
        assert_eq!(
            report.delivered().next().map(|one| one.channel().clone()),
            Some(Channel::SLACK)
        );

        let failure = report.failures().next().expect("discord failed");
        assert_eq!(failure.channel(), &Channel::DISCORD);
        assert!(
            failure
                .error()
                .is_some_and(|error| matches!(error, NotifyError::Rejected { status: 404, .. }))
        );
        assert!(!failure.is_retryable());
    }
}
