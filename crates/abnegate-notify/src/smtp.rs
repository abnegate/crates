//! How to reach an SMTP relay.

use std::fmt;

use abnegate_secret::SecretValue;

#[cfg(feature = "smtp")]
use abnegate_secret::sanitize_owned;
#[cfg(feature = "smtp")]
use lettre::message::Mailbox;
#[cfg(feature = "smtp")]
use lettre::transport::smtp::AsyncSmtpTransportBuilder;
#[cfg(feature = "smtp")]
use lettre::transport::smtp::authentication::Credentials;
#[cfg(feature = "smtp")]
use lettre::{Address, AsyncSmtpTransport, Tokio1Executor};

#[cfg(feature = "smtp")]
use crate::error::Error;

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
    /// A transport builder for this relay, credentials included.
    ///
    /// `relay` sets implicit TLS and port 465. Pointing that at 587, the
    /// submission port every mainstream relay serves with STARTTLS, would send
    /// a ClientHello to a plaintext listener. `starttls_relay` refuses to send
    /// credentials if the upgrade fails, so this stays downgrade-safe.
    ///
    /// A builder rather than a transport: building one is what starts
    /// `lettre`'s connection pool when its `pool` feature is on, and that
    /// needs a running Tokio runtime to construct and to drop.
    pub(crate) fn builder(&self) -> Result<AsyncSmtpTransportBuilder, Error> {
        let builder = if self.port == SUBMISSIONS_PORT {
            AsyncSmtpTransport::<Tokio1Executor>::relay(&self.host)
        } else {
            AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&self.host)
        }
        .map_err(|error| failure(&self.host, error))?;

        let credentials = Credentials::new(self.user.clone(), self.password.expose().to_string());
        Ok(builder.port(self.port).credentials(credentials))
    }

    pub(crate) fn sender(&self) -> Result<Mailbox, Error> {
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

/// Parse `address`, without quoting it back on failure.
///
/// An address is personal data, and the error is headed for a log line.
#[cfg(feature = "smtp")]
pub(crate) fn mailbox(address: &str, name: Option<String>) -> Result<Mailbox, Error> {
    let parsed: Address = address.parse().map_err(|_| Error::Malformed {
        message: "an email address could not be parsed".to_string(),
    })?;
    Ok(Mailbox::new(name, parsed))
}

/// An SMTP failure, with the relay's reply sanitized.
///
/// A relay's reply text is written by the far end, so it is treated like any
/// other outside text on its way to a log line.
#[cfg(feature = "smtp")]
pub(crate) fn failure(host: &str, error: lettre::transport::smtp::Error) -> Error {
    Error::Smtp {
        host: host.to_string(),
        message: sanitize_owned(error.to_string()),
    }
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

    #[cfg(feature = "smtp")]
    #[test]
    fn an_unparseable_address_is_refused_without_being_quoted() {
        let error = mailbox("someone.private@@example.test", None).expect_err("bad address");
        assert!(matches!(error, Error::Malformed { .. }));
        assert!(
            !error.to_string().contains("someone.private"),
            "the address reached the message: {error}"
        );
    }
}
