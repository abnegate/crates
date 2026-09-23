//! The fan-out contract, exercised through the public API.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use abnegate_notify::{Channel, Fanout, Notification, Notifier, NotifyError, Severity};
use async_trait::async_trait;

/// A notifier that records what it was asked to send and then does as told.
struct Stub {
    channel: Channel,
    name: Option<String>,
    outcome: Outcome,
    delivered: Arc<AtomicUsize>,
}

enum Outcome {
    Succeed,
    Fail,
    Hang,
    Panic,
    Slow(Duration),
}

impl Stub {
    fn new(channel: Channel, outcome: Outcome) -> Self {
        Self {
            channel,
            name: None,
            outcome,
            delivered: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn named(mut self, name: &str) -> Self {
        self.name = Some(name.to_string());
        self
    }

    fn counter(&self) -> Arc<AtomicUsize> {
        Arc::clone(&self.delivered)
    }
}

#[async_trait]
impl Notifier for Stub {
    fn channel(&self) -> Channel {
        self.channel.clone()
    }

    fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    async fn deliver(&self, _notification: &Notification) -> Result<(), NotifyError> {
        match &self.outcome {
            Outcome::Succeed => {
                self.delivered.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
            Outcome::Fail => Err(NotifyError::Malformed {
                message: "nope".to_string(),
            }),
            Outcome::Hang => {
                tokio::time::sleep(Duration::from_secs(3_600)).await;
                Ok(())
            }
            Outcome::Slow(duration) => {
                tokio::time::sleep(*duration).await;
                self.delivered.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
            Outcome::Panic => panic!("this backend is broken"),
        }
    }
}

/// A notifier that insists on a budget of its own.
struct Impatient;

#[async_trait]
impl Notifier for Impatient {
    fn channel(&self) -> Channel {
        Channel::custom("impatient")
    }

    fn timeout(&self) -> Option<Duration> {
        Some(Duration::from_millis(50))
    }

    async fn deliver(&self, _notification: &Notification) -> Result<(), NotifyError> {
        tokio::time::sleep(Duration::from_secs(3_600)).await;
        Ok(())
    }
}

fn notification() -> Notification {
    Notification::new("Build failed", "3 tests failed").severity(Severity::Error)
}

#[tokio::test]
async fn an_empty_fanout_is_not_an_error() {
    let report = Fanout::new().deliver(&notification()).await;

    assert!(report.is_empty());
    assert_eq!(report.len(), 0);
    assert!(report.all_delivered());
    assert!(!report.any_delivered());
    assert_eq!(report.failures().count(), 0);
}

#[tokio::test]
async fn every_configured_channel_receives_the_notification() {
    let slack = Stub::new(Channel::SLACK, Outcome::Succeed);
    let discord = Stub::new(Channel::DISCORD, Outcome::Succeed);
    let email = Stub::new(Channel::EMAIL, Outcome::Succeed);
    let counts = [slack.counter(), discord.counter(), email.counter()];

    let report = Fanout::new()
        .with(slack)
        .with(discord)
        .with(email)
        .deliver(&notification())
        .await;

    assert_eq!(report.len(), 3);
    assert!(report.all_delivered());
    for counter in counts {
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    let channels: Vec<String> = report
        .deliveries()
        .iter()
        .map(|one| one.channel().to_string())
        .collect();
    assert_eq!(channels, vec!["slack", "discord", "email"]);
}

#[tokio::test]
async fn one_failing_channel_does_not_stop_the_others() {
    let slack = Stub::new(Channel::SLACK, Outcome::Succeed);
    let email = Stub::new(Channel::EMAIL, Outcome::Succeed);
    let (slack_count, email_count) = (slack.counter(), email.counter());

    let report = Fanout::new()
        .with(slack)
        .with(Stub::new(Channel::DISCORD, Outcome::Fail))
        .with(email)
        .deliver(&notification())
        .await;

    assert_eq!(slack_count.load(Ordering::SeqCst), 1);
    assert_eq!(email_count.load(Ordering::SeqCst), 1);

    assert!(!report.all_delivered());
    assert!(report.any_delivered());
    assert_eq!(report.delivered().count(), 2);

    let failures: Vec<&str> = report
        .failures()
        .map(|one| one.channel().as_str())
        .collect();
    assert_eq!(failures, vec!["discord"]);
}

#[tokio::test]
async fn a_failure_is_reported_with_its_reason() {
    let report = Fanout::new()
        .with(Stub::new(Channel::DISCORD, Outcome::Fail))
        .deliver(&notification())
        .await;

    let failure = report.failures().next().expect("one failure");
    assert_eq!(failure.name(), "discord");
    assert_eq!(
        failure.error().map(ToString::to_string),
        Some("the message could not be built: nope".to_string())
    );
    assert!(!failure.is_retryable());
}

#[tokio::test(start_paused = true)]
async fn a_hanging_channel_is_abandoned_at_the_timeout() {
    let slack = Stub::new(Channel::SLACK, Outcome::Succeed);
    let slack_count = slack.counter();

    let report = Fanout::new()
        .timeout(Duration::from_secs(2))
        .with(Stub::new(Channel::DISCORD, Outcome::Hang))
        .with(slack)
        .deliver(&notification())
        .await;

    assert_eq!(slack_count.load(Ordering::SeqCst), 1);
    assert_eq!(report.len(), 2);

    let timed_out = report.failures().next().expect("one failure");
    assert_eq!(timed_out.channel().as_str(), "discord");
    assert_eq!(
        timed_out.error(),
        Some(&NotifyError::Timeout {
            after: Duration::from_secs(2)
        })
    );
    assert!(timed_out.is_retryable());
}

#[tokio::test(start_paused = true)]
async fn a_channel_inside_the_budget_still_delivers() {
    let slow = Stub::new(Channel::EMAIL, Outcome::Slow(Duration::from_millis(500)));
    let counter = slow.counter();

    let report = Fanout::new()
        .timeout(Duration::from_secs(5))
        .with(slow)
        .deliver(&notification())
        .await;

    assert!(report.all_delivered());
    assert_eq!(counter.load(Ordering::SeqCst), 1);
}

#[tokio::test(start_paused = true)]
async fn a_channel_may_impose_a_tighter_budget_than_the_fanout() {
    let report = Fanout::new()
        .timeout(Duration::from_secs(600))
        .with(Impatient)
        .deliver(&notification())
        .await;

    let failure = report.failures().next().expect("one failure");
    assert_eq!(failure.channel().as_str(), "impatient");
    assert_eq!(
        failure.error(),
        Some(&NotifyError::Timeout {
            after: Duration::from_millis(50)
        })
    );
}

#[tokio::test]
async fn a_panicking_channel_is_contained_and_reported() {
    let slack = Stub::new(Channel::SLACK, Outcome::Succeed);
    let counter = slack.counter();

    let report = Fanout::new()
        .with(Stub::new(Channel::DISCORD, Outcome::Panic))
        .with(slack)
        .deliver(&notification())
        .await;

    assert_eq!(counter.load(Ordering::SeqCst), 1);
    assert_eq!(report.len(), 2);

    let failure = report.failures().next().expect("one failure");
    assert_eq!(failure.channel().as_str(), "discord");
    assert_eq!(failure.error(), Some(&NotifyError::Panicked));
    assert!(!failure.is_retryable());
}

#[tokio::test]
async fn two_instances_of_one_channel_are_told_apart() {
    let report = Fanout::new()
        .with(Stub::new(Channel::SLACK, Outcome::Succeed).named("alerts"))
        .with(Stub::new(Channel::SLACK, Outcome::Fail).named("engineering"))
        .deliver(&notification())
        .await;

    let names: Vec<&str> = report.deliveries().iter().map(|one| one.name()).collect();
    assert_eq!(names, vec!["alerts", "engineering"]);
    assert_eq!(
        report.failures().map(|one| one.name()).collect::<Vec<_>>(),
        vec!["engineering"]
    );
}

#[tokio::test]
async fn deliveries_are_reported_in_registration_order() {
    let mut fanout = Fanout::new();
    for index in 0..8 {
        let outcome = if index % 2 == 0 {
            Outcome::Succeed
        } else {
            Outcome::Fail
        };
        fanout.register(Arc::new(
            Stub::new(Channel::custom("bulk"), outcome).named(&format!("channel-{index}")),
        ));
    }

    let report = fanout.deliver(&notification()).await;
    let names: Vec<&str> = report.deliveries().iter().map(|one| one.name()).collect();

    assert_eq!(
        names,
        vec![
            "channel-0",
            "channel-1",
            "channel-2",
            "channel-3",
            "channel-4",
            "channel-5",
            "channel-6",
            "channel-7"
        ]
    );
    assert_eq!(report.delivered().count(), 4);
    assert_eq!(report.failures().count(), 4);
}

#[tokio::test(start_paused = true)]
async fn channels_run_concurrently_rather_than_one_after_another() {
    let mut fanout = Fanout::new().timeout(Duration::from_secs(30));
    for _ in 0..5 {
        fanout.register(Arc::new(Stub::new(
            Channel::custom("slow"),
            Outcome::Slow(Duration::from_secs(4)),
        )));
    }

    let started = tokio::time::Instant::now();
    let report = fanout.deliver(&notification()).await;
    let elapsed = started.elapsed();

    assert!(report.all_delivered());
    assert!(
        elapsed < Duration::from_secs(20),
        "five 4s channels took {elapsed:?}, so they ran in series"
    );
}
