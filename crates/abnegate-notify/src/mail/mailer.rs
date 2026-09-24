//! Mail through an SMTP relay.

use std::fmt;

use async_trait::async_trait;
use lettre::message::Mailbox;
use lettre::message::header::ContentType;
use lettre::transport::smtp::AsyncSmtpTransportBuilder;
use lettre::{AsyncTransport, Message, Tokio1Executor};

use crate::error::Error;
use crate::mail::Mail;
use crate::smtp::{SmtpConfig, failure, mailbox};

/// A [`Mail`] that sends through an SMTP relay.
///
/// The transport is built for each message rather than held, so a mailer can
/// be constructed and dropped without a Tokio runtime, even in a build where
/// another crate switches on `lettre`'s `pool` feature.
#[cfg_attr(docsrs, doc(cfg(feature = "smtp")))]
pub struct Mailer {
    builder: AsyncSmtpTransportBuilder,
    from: Mailbox,
    host: String,
}

impl Mailer {
    /// Configure delivery through the relay described by `config`.
    ///
    /// Nothing connects until a message is sent.
    pub fn new(config: &SmtpConfig) -> Result<Self, Error> {
        Ok(Self {
            from: config.sender()?,
            builder: config.builder()?,
            host: config.host.clone(),
        })
    }

    /// The relay this mailer sends through, which is safe to log.
    pub fn host(&self) -> &str {
        &self.host
    }

    fn compose(&self, recipient: &str, subject: &str, body: &str) -> Result<Message, Error> {
        Message::builder()
            .from(self.from.clone())
            .to(mailbox(recipient, None)?)
            .subject(subject)
            .header(ContentType::TEXT_PLAIN)
            .body(body.to_string())
            .map_err(|error| Error::Malformed {
                message: error.to_string(),
            })
    }
}

#[async_trait]
impl Mail for Mailer {
    async fn send(&self, recipient: &str, subject: &str, body: &str) -> Result<(), Error> {
        let message = self.compose(recipient, subject, body)?;
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
impl fmt::Debug for Mailer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Mailer")
            .field("host", &self.host)
            .field("from", &self.from)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> SmtpConfig {
        SmtpConfig::new(
            "smtp.example.test",
            587,
            "postmaster",
            "hunter2-not-a-real-password",
            "noreply@example.test",
            "Notifications",
        )
    }

    fn mailer() -> Mailer {
        Mailer::new(&config()).expect("configured")
    }

    fn rendered(recipient: &str, subject: &str, body: &str) -> String {
        String::from_utf8(
            mailer()
                .compose(recipient, subject, body)
                .expect("composed")
                .formatted(),
        )
        .expect("utf-8")
    }

    /// A plain test rather than a Tokio one: no runtime is running here.
    #[test]
    fn a_mailer_is_built_and_dropped_outside_a_runtime() {
        for port in [465, 587] {
            let mut relay = config();
            relay.port = port;
            drop(Mailer::new(&relay).expect("configured"));
        }
    }

    #[test]
    fn an_unparseable_recipient_is_refused() {
        let error = mailer()
            .compose("not an address", "Subject", "Body")
            .expect_err("bad recipient");
        assert!(matches!(error, Error::Malformed { .. }));
    }

    #[test]
    fn an_unparseable_sender_is_refused() {
        let mut broken = config();
        broken.from_address = "@@@".to_string();
        assert!(matches!(
            Mailer::new(&broken).expect_err("bad sender"),
            Error::Malformed { .. }
        ));
    }

    #[test]
    fn the_message_carries_the_subject_body_and_both_addresses() {
        let message = rendered(
            "person@example.test",
            "Verify your email address",
            "Open https://example.test/verify?t=abc",
        );

        assert!(message.contains("Subject: Verify your email address"));
        assert!(message.contains("From: Notifications <noreply@example.test>"));
        assert!(message.contains("To: person@example.test"));
        assert!(message.contains("Content-Type: text/plain"));
        assert!(message.contains("https://example.test/verify?t=abc"));
    }

    /// The reason [`Mailer`] does not redact what it is handed.
    #[test]
    fn a_one_time_token_reaches_the_recipient_intact() {
        let reset = "https://example.test/reset?token=8Xk2Qm9Lp4Rw7Tz1Vb6Nh3Yj5Fd0Gs8Ac2Ee4Ii6Ko";
        let message = rendered("person@example.test", "Reset your password", reset);
        assert!(message.contains(reset), "the link was mangled: {message}");
    }

    #[test]
    fn the_relay_password_never_reaches_the_message_or_debug() {
        let message = rendered("person@example.test", "Subject", "Body");
        assert!(!message.contains("hunter2"), "leaked into the message");

        let debug = format!("{:?}", mailer());
        assert!(!debug.contains("hunter2"), "leaked: {debug}");
        assert!(debug.contains("smtp.example.test"));
    }

    #[test]
    fn the_relay_is_reported() {
        assert_eq!(mailer().host(), "smtp.example.test");
    }
}
