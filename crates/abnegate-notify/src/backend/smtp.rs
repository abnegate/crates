//! How to reach an SMTP relay, and the transport that talks to one.

use std::fmt;
use std::sync::Arc;

use abnegate_secret::SecretValue;
use tokio::sync::Mutex;

#[cfg(feature = "smtp")]
use abnegate_secret::sanitize_owned;
#[cfg(feature = "smtp")]
use lettre::message::Mailbox;
#[cfg(feature = "smtp")]
use lettre::message::header::ContentType;
#[cfg(feature = "smtp")]
use lettre::transport::smtp::authentication::Credentials;
#[cfg(feature = "smtp")]
use lettre::{Address, AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};

use crate::error::NotifyError;

/// Implicit-TLS submissions port, the one port `relay` is built for.
#[cfg(feature = "smtp")]
const SUBMISSIONS_PORT: u16 = 465;

/// The connection details for one SMTP relay.
///
/// The password is a [`SecretValue`], so it is redacted in `Debug`, zeroized
/// when the config is dropped, and readable only at the point it is handed to
/// the transport.
#[derive(Clone)]
pub struct SmtpConfig {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: SecretValue,
    pub from_address: String,
    pub from_name: String,
}

#[cfg(feature = "smtp")]
impl SmtpConfig {
    /// A transport for this relay.
    ///
    /// `relay` sets implicit TLS and port 465. Pointing that at 587, the
    /// submission port every mainstream relay serves with STARTTLS, would send
    /// a ClientHello to a plaintext listener. `starttls_relay` refuses to send
    /// credentials if the upgrade fails, so this stays downgrade-safe.
    pub(crate) fn transport(&self) -> Result<AsyncSmtpTransport<Tokio1Executor>, NotifyError> {
        let builder = if self.port == SUBMISSIONS_PORT {
            AsyncSmtpTransport::<Tokio1Executor>::relay(&self.host)
        } else {
            AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&self.host)
        };

        let credentials = Credentials::new(self.user.clone(), self.password.expose().to_string());

        Ok(builder
            .map_err(|error| NotifyError::Smtp {
                host: self.host.clone(),
                message: describe(error),
            })?
            .port(self.port)
            .credentials(credentials)
            .build())
    }

    pub(crate) fn sender(&self) -> Result<Mailbox, NotifyError> {
        mailbox(&self.from_address, Some(self.from_name.clone()))
    }
}

impl fmt::Debug for SmtpConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SmtpConfig")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("user", &self.user)
            .field("password", &self.password)
            .field("from_address", &self.from_address)
            .field("from_name", &self.from_name)
            .finish()
    }
}

/// Sends one arbitrary message through an SMTP relay.
///
/// This is the transactional half of the crate: a subject and a body for one
/// recipient, with no [`Notification`](crate::Notification) involved. Use
/// [`Email`](crate::Email) instead to make a relay one channel of a fan-out.
///
/// Unlike a notification, the text here is sent exactly as it was given.
/// Redaction would eat the one-time token in an account-flow link, which is
/// the whole payload of the message it appears in.
///
/// Construct and drop this inside a Tokio runtime. When `lettre`'s `pool`
/// feature is on anywhere in the build, its connection pool spawns a task
/// from its own `Drop`, and a panic in a destructor aborts the process rather
/// than unwinding.
#[cfg(feature = "smtp")]
#[cfg_attr(docsrs, doc(cfg(feature = "smtp")))]
pub struct Mailer {
    transport: AsyncSmtpTransport<Tokio1Executor>,
    from: Mailbox,
    host: String,
}

#[cfg(feature = "smtp")]
impl Mailer {
    /// Connect to the relay described by `config`.
    ///
    /// The sender is parsed first so that a bad address never builds a
    /// transport: dropping one outside a Tokio runtime aborts the process.
    pub fn new(config: &SmtpConfig) -> Result<Self, NotifyError> {
        let from = config.sender()?;
        Ok(Self {
            transport: config.transport()?,
            from,
            host: config.host.clone(),
        })
    }

    /// The relay this mailer sends through, which is safe to log.
    pub fn host(&self) -> &str {
        &self.host
    }

    pub async fn send(
        &self,
        recipient: &str,
        subject: &str,
        body: &str,
    ) -> Result<(), NotifyError> {
        let message = self.compose(recipient, subject, body)?;
        self.transport
            .send(message)
            .await
            .map_err(|error| NotifyError::Smtp {
                host: self.host.clone(),
                message: describe(error),
            })?;
        Ok(())
    }

    fn compose(&self, recipient: &str, subject: &str, body: &str) -> Result<Message, NotifyError> {
        Message::builder()
            .from(self.from.clone())
            .to(mailbox(recipient, None)?)
            .subject(subject)
            .header(ContentType::TEXT_PLAIN)
            .body(body.to_string())
            .map_err(|error| NotifyError::Malformed {
                message: error.to_string(),
            })
    }
}

/// Written out rather than derived: the transport holds the relay password
/// inside a `lettre::Credentials`, which has no redacting `Debug` of its own.
#[cfg(feature = "smtp")]
impl fmt::Debug for Mailer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Mailer")
            .field("host", &self.host)
            .field("from", &self.from)
            .finish_non_exhaustive()
    }
}

/// A [`Mailer`](crate::Mailer) that records instead of sending.
///
/// It takes and answers exactly what the real one does, so a caller under test
/// can hold either and assert on what would have gone out. Available without
/// the `smtp` feature, so a downstream test suite need not build `lettre`.
#[derive(Clone, Debug, Default)]
pub struct MockMailer {
    sent: Arc<Mutex<Vec<SentMail>>>,
}

impl MockMailer {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn send(
        &self,
        recipient: &str,
        subject: &str,
        body: &str,
    ) -> Result<(), NotifyError> {
        self.sent.lock().await.push(SentMail {
            recipient: recipient.to_string(),
            subject: subject.to_string(),
            body: body.to_string(),
        });
        Ok(())
    }

    /// Everything sent so far, oldest first.
    pub async fn sent(&self) -> Vec<SentMail> {
        self.sent.lock().await.clone()
    }

    pub async fn clear(&self) {
        self.sent.lock().await.clear();
    }
}

/// One message a [`MockMailer`] was asked to send.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SentMail {
    pub recipient: String,
    pub subject: String,
    pub body: String,
}

#[cfg(feature = "smtp")]
pub(crate) fn mailbox(address: &str, name: Option<String>) -> Result<Mailbox, NotifyError> {
    let parsed: Address = address.parse().map_err(|_| NotifyError::Malformed {
        message: format!("{address} is not a valid email address"),
    })?;
    Ok(Mailbox::new(name, parsed))
}

/// An SMTP error's message, sanitized.
///
/// A relay's reply text is written by the far end, so it is treated like any
/// other outside text on its way to a log line.
#[cfg(feature = "smtp")]
pub(crate) fn describe(error: lettre::transport::smtp::Error) -> String {
    sanitize_owned(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn the_password_never_appears_in_debug() {
        let rendered = format!("{:?}", config());
        assert!(!rendered.contains("hunter2"), "leaked: {rendered}");
        assert!(rendered.contains("[REDACTED]"));
        assert!(rendered.contains("smtp.example.test"));
    }

    #[test]
    fn the_password_is_still_readable_at_the_point_of_use() {
        assert_eq!(config().password.expose(), "hunter2-not-a-real-password");
    }

    #[tokio::test]
    async fn the_mock_records_every_message_it_was_given() {
        let mailer = MockMailer::new();

        mailer
            .send(
                "person@example.test",
                "Verify your email address",
                "Open https://example.test/verify?t=abc",
            )
            .await
            .expect("recorded");
        mailer
            .send(
                "person@example.test",
                "Reset your password",
                "Open https://example.test/reset?t=abc",
            )
            .await
            .expect("recorded");

        let sent = mailer.sent().await;
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[0].recipient, "person@example.test");
        assert_eq!(sent[0].subject, "Verify your email address");
        assert_eq!(sent[1].subject, "Reset your password");
    }

    #[tokio::test]
    async fn the_mock_can_be_cleared() {
        let mailer = MockMailer::new();

        mailer
            .send("person@example.test", "Subject", "Body")
            .await
            .expect("recorded");
        assert_eq!(mailer.sent().await.len(), 1);

        mailer.clear().await;
        assert_eq!(mailer.sent().await.len(), 0);
    }

    #[cfg(feature = "smtp")]
    mod transport {
        use super::*;

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

        #[tokio::test]
        async fn an_unparseable_recipient_is_refused() {
            let error = mailer()
                .compose("not an address", "Subject", "Body")
                .expect_err("bad recipient");
            assert!(matches!(error, NotifyError::Malformed { .. }));
        }

        #[tokio::test]
        async fn an_unparseable_sender_is_refused() {
            let mut broken = config();
            broken.from_address = "@@@".to_string();
            assert!(matches!(
                Mailer::new(&broken).expect_err("bad sender"),
                NotifyError::Malformed { .. }
            ));
        }

        #[tokio::test]
        async fn the_message_carries_the_subject_body_and_both_addresses() {
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
        #[tokio::test]
        async fn a_one_time_token_reaches_the_recipient_intact() {
            let reset =
                "https://example.test/reset?token=8Xk2Qm9Lp4Rw7Tz1Vb6Nh3Yj5Fd0Gs8Ac2Ee4Ii6Ko";
            let message = rendered("person@example.test", "Reset your password", reset);
            assert!(message.contains(reset), "the link was mangled: {message}");
        }

        #[tokio::test]
        async fn the_relay_password_never_reaches_the_message_or_debug() {
            let message = rendered("person@example.test", "Subject", "Body");
            assert!(!message.contains("hunter2"), "leaked into the message");

            let debug = format!("{:?}", mailer());
            assert!(!debug.contains("hunter2"), "leaked: {debug}");
            assert!(debug.contains("smtp.example.test"));
        }

        #[tokio::test]
        async fn the_relay_is_reported() {
            assert_eq!(mailer().host(), "smtp.example.test");
        }
    }
}
