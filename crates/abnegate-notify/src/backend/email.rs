//! Email, over SMTP.

use std::fmt;

use async_trait::async_trait;
use lettre::message::Mailbox;
use lettre::message::header::ContentType;
use lettre::transport::smtp::AsyncSmtpTransportBuilder;
use lettre::{AsyncTransport, Message, Tokio1Executor};

use crate::channel::Channel;
use crate::error::Error;
use crate::notification::Notification;
use crate::notifier::Notifier;
use crate::smtp::{SmtpConfig, failure, mailbox};

/// Delivers to a fixed set of recipients through one SMTP relay.
///
/// The transport is built for each delivery rather than held, so this can be
/// constructed and dropped without a Tokio runtime, even in a build where
/// another crate switches on `lettre`'s `pool` feature.
#[cfg_attr(docsrs, doc(cfg(feature = "smtp")))]
pub struct Email {
    builder: AsyncSmtpTransportBuilder,
    from: Mailbox,
    recipients: Vec<Mailbox>,
    host: String,
    name: Option<String>,
}

impl Email {
    /// Configure delivery through the relay in `config` to `recipients`.
    ///
    /// Nothing connects until a notification is delivered.
    pub fn new(config: &SmtpConfig, recipients: &[&str]) -> Result<Self, Error> {
        if recipients.is_empty() {
            return Err(Error::Malformed {
                message: "an email channel needs at least one recipient".to_string(),
            });
        }

        let from = config.sender()?;
        let recipients = recipients
            .iter()
            .map(|recipient| mailbox(recipient, None))
            .collect::<Result<Vec<Mailbox>, Error>>()?;

        Ok(Self {
            builder: config.builder()?,
            from,
            recipients,
            host: config.host.clone(),
            name: None,
        })
    }

    /// Label this instance, for a caller with more than one recipient set.
    #[must_use]
    pub fn named(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    fn compose(&self, notification: &Notification) -> Result<Message, Error> {
        let mut builder = Message::builder()
            .from(self.from.clone())
            .subject(notification.title());

        for recipient in &self.recipients {
            builder = builder.to(recipient.clone());
        }

        builder
            .header(ContentType::TEXT_PLAIN)
            .body(notification.to_plain_text())
            .map_err(|error| Error::Malformed {
                message: error.to_string(),
            })
    }
}

#[async_trait]
impl Notifier for Email {
    fn channel(&self) -> Channel {
        Channel::EMAIL
    }

    fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    async fn deliver(&self, notification: &Notification) -> Result<(), Error> {
        let message = self.compose(notification)?;
        self.builder
            .clone()
            .build::<Tokio1Executor>()
            .send(message)
            .await
            .map_err(|error| failure(&self.host, error))?;
        Ok(())
    }
}

/// Written out rather than derived: the builder holds the relay password
/// inside a `lettre::Credentials`, which has no redacting `Debug` of its own.
impl fmt::Debug for Email {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Email")
            .field("host", &self.host)
            .field("from", &self.from)
            .field("recipients", &self.recipients)
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::severity::Severity;
    use abnegate_secret::SecretValue;

    fn config() -> SmtpConfig {
        SmtpConfig {
            host: "smtp.example.test".to_string(),
            port: 587,
            user: "postmaster".to_string(),
            password: SecretValue::new("hunter2-not-a-real-password"),
            from_address: "noreply@example.test".to_string(),
            from_name: "Notifications".to_string(),
        }
    }

    fn email() -> Email {
        Email::new(&config(), &["sam@example.test"]).expect("configured")
    }

    fn rendered(notification: &Notification) -> String {
        String::from_utf8(email().compose(notification).expect("composed").formatted())
            .expect("utf-8")
    }

    /// A plain test rather than a Tokio one: no runtime is running here.
    #[test]
    fn a_channel_is_built_and_dropped_outside_a_runtime() {
        for port in [465, 587] {
            let mut relay = config();
            relay.port = port;
            drop(Email::new(&relay, &["sam@example.test"]).expect("configured"));
        }
    }

    #[test]
    fn a_channel_with_no_recipients_is_refused() {
        let error = Email::new(&config(), &[]).expect_err("no recipients");
        assert!(matches!(error, Error::Malformed { .. }));
        assert!(error.to_string().contains("at least one recipient"));
    }

    #[test]
    fn an_unparseable_recipient_is_refused() {
        let error = Email::new(&config(), &["not an address"]).expect_err("bad recipient");
        assert!(matches!(error, Error::Malformed { .. }));
    }

    #[test]
    fn an_unparseable_sender_is_refused() {
        let mut broken = config();
        broken.from_address = "@@@".to_string();
        assert!(matches!(
            Email::new(&broken, &["sam@example.test"]).expect_err("bad sender"),
            Error::Malformed { .. }
        ));
    }

    #[test]
    fn the_subject_is_the_title_and_the_body_carries_everything() {
        let message = rendered(
            &Notification::new("Build failed", "3 tests failed")
                .severity(Severity::Error)
                .field("Branch", "main")
                .link("https://example.test/b/1"),
        );

        assert!(message.contains("Subject: Build failed"));
        assert!(message.contains("From: Notifications <noreply@example.test>"));
        assert!(message.contains("To: sam@example.test"));
        assert!(message.contains("Content-Type: text/plain"));
        assert!(message.contains("3 tests failed"));
        assert!(message.contains("Branch: main"));
        assert!(message.contains("https://example.test/b/1"));
    }

    #[test]
    fn every_recipient_is_addressed() {
        let email =
            Email::new(&config(), &["sam@example.test", "alex@example.test"]).expect("configured");
        let message = String::from_utf8(
            email
                .compose(&Notification::new("Title", "Body"))
                .expect("composed")
                .formatted(),
        )
        .expect("utf-8");

        assert!(message.contains("sam@example.test"));
        assert!(message.contains("alex@example.test"));
    }

    #[test]
    fn the_relay_password_never_reaches_the_message() {
        let message = rendered(&Notification::new("Title", "Body"));
        assert!(!message.contains("hunter2"), "leaked into the message");
    }

    #[test]
    fn a_credential_in_the_body_is_redacted_before_it_is_composed() {
        let message = rendered(&Notification::new(
            "Deploy log",
            concat!("GITHUB_TOKEN=ghp_", "0123456789abcdefghij"),
        ));
        assert!(!message.contains(concat!("ghp_", "0123456789abcdefghij")));
        assert!(message.contains("[REDACTED]"));
    }

    #[test]
    fn the_relay_password_never_appears_in_debug() {
        let rendered = format!("{:?}", email());
        assert!(!rendered.contains("hunter2"), "leaked: {rendered}");
        assert!(rendered.contains("smtp.example.test"));
    }

    #[test]
    fn the_channel_and_name_are_reported() {
        assert_eq!(email().channel(), Channel::EMAIL);
        assert_eq!(email().name(), None);
        assert_eq!(email().named("ops").name(), Some("ops"));
    }
}
