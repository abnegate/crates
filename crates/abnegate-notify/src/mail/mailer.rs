//! Mail through an SMTP relay.

use std::fmt;

use async_trait::async_trait;
use lettre::AsyncTransport;
use lettre::Message;
use lettre::Tokio1Executor;
use lettre::message::Mailbox;
use lettre::message::header::ContentType;
use lettre::transport::smtp::AsyncSmtpTransportBuilder;

use crate::error::Error;
use crate::mail::Mail;
use crate::smtp::SmtpConfig;
use crate::smtp::failure;
use crate::smtp::parse_mailbox;

/// A [`Mail`] that sends through an SMTP relay.
///
/// A recipient is a bare address or a `Name <address>` mailbox.
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
            from: config.sender.mailbox()?,
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
            .to(parse_mailbox(recipient)?)
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
    use crate::smtp::Sender;
    use crate::smtp::SmtpSecurity;

    fn config_from(sender: Sender) -> SmtpConfig {
        SmtpConfig::new(
            "smtp.example.test",
            587,
            "postmaster",
            "hunter2-not-a-real-password",
            sender,
        )
    }

    fn config() -> SmtpConfig {
        config_from(Sender::new("noreply@example.test").with_name("Notifications"))
    }

    fn mailer() -> Mailer {
        Mailer::new(&config()).expect("configured")
    }

    fn rendered(recipient: &str, subject: &str, body: &str) -> String {
        rendered_by(&mailer(), recipient, subject, body)
    }

    fn rendered_by(mailer: &Mailer, recipient: &str, subject: &str, body: &str) -> String {
        String::from_utf8(
            mailer
                .compose(recipient, subject, body)
                .expect("composed")
                .formatted(),
        )
        .expect("utf-8")
    }

    fn sent_as(sender: Sender) -> String {
        let mailer = Mailer::new(&config_from(sender)).expect("configured");
        rendered_by(&mailer, "person@example.test", "Subject", "Body")
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
        let broken = config_from(Sender::new("@@@"));
        assert!(matches!(
            Mailer::new(&broken).expect_err("bad sender"),
            Error::Malformed { .. }
        ));
    }

    #[test]
    fn a_named_recipient_is_addressed_by_name() {
        let message = rendered("Sam Example <person@example.test>", "Subject", "Body");
        assert!(
            message.contains("To: \"Sam Example\" <person@example.test>\r\n"),
            "{message}"
        );
    }

    #[test]
    fn a_line_break_in_a_recipient_name_never_reaches_the_headers() {
        for recipient in [
            "Sam\r\nBcc: hidden@example.test <person@example.test>",
            "Sam\r\n Bcc: hidden@example.test <person@example.test>",
            "\"Sam\r\nBcc: hidden@example.test\" <person@example.test>",
            "Sam\nBcc: hidden@example.test <person@example.test>",
        ] {
            let error = mailer()
                .compose(recipient, "Subject", "Body")
                .expect_err(recipient);
            assert!(matches!(error, Error::Malformed { .. }), "{recipient:?}");
        }
    }

    #[test]
    fn an_encoded_line_break_stays_inside_the_name() {
        let message = rendered(
            "=?utf-8?b?U2FtDQpCY2M6IGhpZGRlbkBleGFtcGxlLnRlc3Q=?= <person@example.test>",
            "Subject",
            "Body",
        );
        let headers = message.split("\r\n\r\n").next().expect("headers");
        assert!(
            !headers
                .split("\r\n")
                .any(|line| line.to_ascii_lowercase().starts_with("bcc:")),
            "{headers}"
        );
        assert!(!headers.contains("hidden@example.test"), "{headers}");
    }

    #[test]
    fn a_line_break_in_the_sender_name_is_refused_rather_than_panicking() {
        for name in [
            "Alerts\r\nBcc: hidden@example.test",
            "Alerts\nBcc: x",
            "Alerts\r",
        ] {
            let broken = config_from(Sender::new("alerts@example.test").with_name(name));
            assert!(
                matches!(Mailer::new(&broken), Err(Error::Malformed { .. })),
                "{name:?}"
            );
        }
    }

    #[test]
    fn a_half_written_mailbox_is_refused() {
        for recipient in [
            "Sam <person@example.test",
            "Sam person@example.test>",
            "Sam <>",
        ] {
            let error = mailer()
                .compose(recipient, "Subject", "Body")
                .expect_err(recipient);
            assert!(matches!(error, Error::Malformed { .. }), "{recipient}");
        }
    }

    #[test]
    fn a_mailer_is_built_for_every_security_and_without_credentials() {
        for security in [
            SmtpSecurity::Implicit,
            SmtpSecurity::StartTls,
            SmtpSecurity::Plaintext,
        ] {
            let relay = config().with_security(security);
            drop(Mailer::new(&relay).expect("signs in"));
            drop(Mailer::new(&relay.without_credentials()).expect("anonymous"));
        }
    }

    #[test]
    fn the_sender_is_the_from_header() {
        let message = sent_as(Sender::new("alerts@example.test").with_name("Build Alerts"));
        assert!(
            message.contains("From: \"Build Alerts\" <alerts@example.test>\r\n"),
            "{message}"
        );
    }

    #[test]
    fn a_sender_without_a_name_is_the_bare_address() {
        let message = sent_as(Sender::new("alerts@example.test"));
        assert!(
            message.contains("From: alerts@example.test\r\n"),
            "{message}"
        );
    }

    #[test]
    fn a_blank_name_is_left_out() {
        for name in ["", "   "] {
            let message = sent_as(Sender::new("alerts@example.test").with_name(name));
            assert!(
                message.contains("From: alerts@example.test\r\n"),
                "{name:?}: {message}"
            );
        }
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
