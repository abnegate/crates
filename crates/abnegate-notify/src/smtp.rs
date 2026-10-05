//! How to reach an SMTP relay.

mod security;
mod sender;

use std::fmt;

use abnegate_secret::SecretValue;

#[cfg(feature = "smtp")]
use abnegate_secret::sanitize_owned;
#[cfg(feature = "smtp")]
use lettre::Address;
#[cfg(feature = "smtp")]
use lettre::AsyncSmtpTransport;
#[cfg(feature = "smtp")]
use lettre::Tokio1Executor;
#[cfg(feature = "smtp")]
use lettre::message::Mailbox;
#[cfg(feature = "smtp")]
use lettre::transport::smtp::AsyncSmtpTransportBuilder;
#[cfg(feature = "smtp")]
use lettre::transport::smtp::authentication::Credentials;

#[cfg(feature = "smtp")]
use crate::error::Error;

pub use crate::smtp::security::SmtpSecurity;
pub use crate::smtp::sender::Sender;

/// The connection details for one SMTP relay.
///
/// The password is a [`SecretValue`], so it is redacted in `Debug`, zeroized
/// when the config is dropped, and readable only at the point it is handed to
/// the transport.
#[derive(Clone)]
#[non_exhaustive]
pub struct SmtpConfig {
    /// The relay's host name, which is safe to log.
    pub host: String,
    /// 465 for implicit TLS; any other port, usually 587, must upgrade with
    /// STARTTLS before the credentials are sent, unless `security` says
    /// otherwise.
    pub port: u16,
    /// The name the relay signs in with. Empty when `authenticate` is off.
    pub user: String,
    /// The relay password. Empty when `authenticate` is off.
    pub password: SecretValue,
    /// Who every message is sent from.
    pub sender: Sender,
    /// How the connection is secured. `None` uses the one `port` implies,
    /// from [`SmtpSecurity::for_port`].
    pub security: Option<SmtpSecurity>,
    /// Whether the relay is signed in to with `user` and `password`. Off, the
    /// relay is sent to without signing in, for one that trusts the network it
    /// is reached from.
    pub authenticate: bool,
}

impl SmtpConfig {
    /// The relay at `host` and `port`, signed in to as `user` with `password`,
    /// sending every message as `sender`.
    ///
    /// Nothing is checked or connected here: the addresses are parsed when a
    /// `Mailer` or an `Email` channel is built from this, and the relay is
    /// reached only when a message is sent.
    ///
    /// ```
    /// use abnegate_notify::Sender;
    /// use abnegate_notify::SmtpConfig;
    ///
    /// let relay = SmtpConfig::new(
    ///     "smtp.example.test",
    ///     587,
    ///     "postmaster",
    ///     "relay-password",
    ///     Sender::new("noreply@example.test").with_name("Notifications"),
    /// );
    /// assert_eq!(relay.host, "smtp.example.test");
    /// assert_eq!(relay.sender.address, "noreply@example.test");
    /// ```
    pub fn new(
        host: impl Into<String>,
        port: u16,
        user: impl Into<String>,
        password: impl Into<SecretValue>,
        sender: Sender,
    ) -> Self {
        Self {
            host: host.into(),
            port,
            user: user.into(),
            password: password.into(),
            sender,
            security: None,
            authenticate: true,
        }
    }

    /// The relay at `host` and `port`, sent to without signing in, sending
    /// every message as `sender`.
    ///
    /// ```
    /// use abnegate_notify::Sender;
    /// use abnegate_notify::SmtpConfig;
    /// use abnegate_notify::SmtpSecurity;
    ///
    /// let relay = SmtpConfig::unauthenticated(
    ///     "relay.internal.test",
    ///     25,
    ///     Sender::new("noreply@example.test"),
    /// )
    /// .with_security(SmtpSecurity::Plaintext);
    /// assert!(!relay.authenticate);
    /// assert_eq!(relay.effective_security(), SmtpSecurity::Plaintext);
    /// ```
    pub fn unauthenticated(host: impl Into<String>, port: u16, sender: Sender) -> Self {
        Self::new(host, port, String::new(), String::new(), sender).without_credentials()
    }

    /// Secure the connection with `security` rather than the one the port
    /// implies.
    ///
    /// This is the only way to choose [`SmtpSecurity::Plaintext`].
    ///
    /// **With [`SmtpSecurity::Plaintext`], a config that signs in sends the
    /// user and password in the clear**, readable by anything on the path to
    /// the relay. Pair it with [`SmtpConfig::without_credentials`] unless the
    /// relay is on loopback or a network trusted end to end.
    #[must_use]
    pub fn with_security(mut self, security: SmtpSecurity) -> Self {
        self.security = Some(security);
        self
    }

    /// Send without signing in, dropping the user and password.
    #[must_use]
    pub fn without_credentials(mut self) -> Self {
        self.user = String::new();
        self.password = SecretValue::new(String::new());
        self.authenticate = false;
        self
    }

    /// How the connection is secured: the security chosen with
    /// [`SmtpConfig::with_security`], or the one the port implies.
    pub fn effective_security(&self) -> SmtpSecurity {
        self.security
            .unwrap_or_else(|| SmtpSecurity::for_port(self.port))
    }
}

#[cfg(feature = "smtp")]
impl SmtpConfig {
    /// A transport builder for this relay, credentials included when it signs
    /// in.
    ///
    /// Implicit TLS is never pointed at 587 by default: that would send a
    /// ClientHello to a plaintext listener. STARTTLS refuses to send
    /// credentials if the upgrade fails, so the default stays downgrade-safe.
    ///
    /// A builder rather than a transport: building one is what starts
    /// `lettre`'s connection pool when its `pool` feature is on, and that
    /// needs a running Tokio runtime to construct and to drop.
    pub(crate) fn builder(&self) -> Result<AsyncSmtpTransportBuilder, Error> {
        let builder = match self.effective_security() {
            SmtpSecurity::Implicit => AsyncSmtpTransport::<Tokio1Executor>::relay(&self.host),
            SmtpSecurity::StartTls => {
                AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&self.host)
            }
            SmtpSecurity::Plaintext => Ok(AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(
                &self.host,
            )),
        }
        .map_err(|error| failure(&self.host, error))?
        .port(self.port);

        if !self.authenticate {
            return Ok(builder);
        }
        let credentials = Credentials::new(self.user.clone(), self.password.expose().to_string());
        Ok(builder.credentials(credentials))
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
            .field("sender", &self.sender)
            .field("security", &self.security)
            .field("authenticate", &self.authenticate)
            .finish()
    }
}

/// Parse `address`, without quoting it back on failure.
///
/// An address is personal data, and the error is headed for a log line.
///
/// A name with a line break is refused: `lettre` cannot encode one, and
/// panics on it when the message is formatted.
#[cfg(feature = "smtp")]
pub(crate) fn mailbox(address: &str, name: Option<String>) -> Result<Mailbox, Error> {
    if name.as_deref().is_some_and(breaks_a_line) {
        return Err(Error::Malformed {
            message: "a display name contains a line break".to_string(),
        });
    }
    let parsed: Address = address.parse().map_err(|_| unparseable())?;
    Ok(Mailbox::new(name, parsed))
}

#[cfg(feature = "smtp")]
fn breaks_a_line(text: &str) -> bool {
    text.contains(['\r', '\n'])
}

/// Parse a bare address or a `Name <address>` mailbox, without quoting it
/// back on failure.
///
/// A bare address is parsed exactly as [`mailbox`] parses it, so only text
/// that it refuses is read as a mailbox.
#[cfg(feature = "smtp")]
pub(crate) fn parse_mailbox(text: &str) -> Result<Mailbox, Error> {
    if let Ok(address) = text.parse::<Address>() {
        return Ok(Mailbox::new(None, address));
    }
    text.parse::<Mailbox>().map_err(|_| unparseable())
}

#[cfg(feature = "smtp")]
fn unparseable() -> Error {
    Error::Malformed {
        message: "an email address could not be parsed".to_string(),
    }
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

    fn sender() -> Sender {
        Sender::new("noreply@example.test").with_name("Notifications")
    }

    fn config() -> SmtpConfig {
        SmtpConfig::new(
            "smtp.example.test",
            587,
            "postmaster",
            "hunter2-not-a-real-password",
            sender(),
        )
    }

    #[test]
    fn each_part_is_kept_where_it_was_given() {
        let config = config();
        assert_eq!(config.host, "smtp.example.test");
        assert_eq!(config.port, 587);
        assert_eq!(config.user, "postmaster");
        assert_eq!(config.sender, sender());
    }

    #[test]
    fn a_new_config_signs_in_and_takes_its_security_from_the_port() {
        let config = config();
        assert!(config.authenticate);
        assert_eq!(config.security, None);
        assert_eq!(config.effective_security(), SmtpSecurity::StartTls);
    }

    #[test]
    fn port_465_means_implicit_tls_and_587_means_starttls() {
        let mut relay = config();
        relay.port = 465;
        assert_eq!(relay.effective_security(), SmtpSecurity::Implicit);
        relay.port = 587;
        assert_eq!(relay.effective_security(), SmtpSecurity::StartTls);
    }

    #[test]
    fn a_chosen_security_overrides_the_port() {
        let relay = config().with_security(SmtpSecurity::Implicit);
        assert_eq!(relay.security, Some(SmtpSecurity::Implicit));
        assert_eq!(relay.effective_security(), SmtpSecurity::Implicit);

        let relay = config().with_security(SmtpSecurity::Plaintext);
        assert_eq!(relay.effective_security(), SmtpSecurity::Plaintext);
    }

    #[test]
    fn plaintext_is_never_chosen_without_asking() {
        for port in [25, 465, 587, 2525] {
            let mut relay = config();
            relay.port = port;
            assert_ne!(relay.effective_security(), SmtpSecurity::Plaintext);
        }
    }

    #[test]
    fn without_credentials_drops_the_user_and_password() {
        let relay = config().without_credentials();
        assert!(!relay.authenticate);
        assert_eq!(relay.user, "");
        assert!(relay.password.is_empty());
        assert!(!format!("{relay:?}").contains("hunter2"));
    }

    #[test]
    fn an_unauthenticated_relay_keeps_everything_else() {
        let relay = SmtpConfig::unauthenticated("relay.example.test", 25, sender());
        assert!(!relay.authenticate);
        assert_eq!(relay.host, "relay.example.test");
        assert_eq!(relay.port, 25);
        assert_eq!(relay.sender, sender());
        assert_eq!(relay.effective_security(), SmtpSecurity::StartTls);
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
    fn every_security_builds_with_and_without_credentials() {
        for security in [
            SmtpSecurity::Implicit,
            SmtpSecurity::StartTls,
            SmtpSecurity::Plaintext,
        ] {
            config()
                .with_security(security)
                .builder()
                .expect("signs in");
            config()
                .with_security(security)
                .without_credentials()
                .builder()
                .expect("anonymous");
        }
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

    #[cfg(feature = "smtp")]
    #[test]
    fn a_name_with_a_line_break_is_refused() {
        for name in ["Sam\r\nBcc: hidden@example.test", "Sam\n", "\rSam"] {
            let error = mailbox("person@example.test", Some(name.to_string())).expect_err(name);
            assert!(matches!(error, Error::Malformed { .. }), "{name:?}");
        }
        assert!(mailbox("person@example.test", Some("Sam\tExample".to_string())).is_ok());
    }

    #[cfg(feature = "smtp")]
    #[test]
    fn a_bare_address_parses_to_a_mailbox_without_a_name() {
        let parsed = parse_mailbox("person@example.test").expect("parsed");
        assert_eq!(parsed.name, None);
        assert_eq!(parsed.email.to_string(), "person@example.test");
    }

    #[cfg(feature = "smtp")]
    #[test]
    fn a_named_mailbox_parses_to_its_name_and_address() {
        for (text, name) in [
            ("Sam Example <person@example.test>", "Sam Example"),
            ("\"Example, Sam\" <person@example.test>", "Example, Sam"),
            ("  Sam <person@example.test>  ", "Sam"),
        ] {
            let parsed = parse_mailbox(text).expect(text);
            assert_eq!(parsed.name.as_deref(), Some(name), "{text}");
            assert_eq!(parsed.email.to_string(), "person@example.test", "{text}");
        }
    }

    #[cfg(feature = "smtp")]
    #[test]
    fn an_unparseable_mailbox_is_refused_without_being_quoted() {
        for text in [
            "",
            "someone.private",
            "Someone Private <someone.private@example.test",
            "Someone Private someone.private@example.test>",
            "Someone Private <>",
            "Someone Private <someone.private@@example.test>",
            "<someone.private@example.test> trailing",
        ] {
            let error = parse_mailbox(text).expect_err(text);
            assert!(matches!(error, Error::Malformed { .. }), "{text}");
            assert!(
                !error.to_string().contains("someone.private"),
                "the address reached the message: {error}"
            );
        }
    }
}
