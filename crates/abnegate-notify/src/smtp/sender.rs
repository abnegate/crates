#[cfg(feature = "smtp")]
use lettre::message::Mailbox;

#[cfg(feature = "smtp")]
use crate::error::Error;
#[cfg(feature = "smtp")]
use crate::smtp::mailbox;
#[cfg(feature = "smtp")]
use crate::smtp::parse_mailbox;

/// Who every message is sent from: an address, and optionally the name
/// shown beside it.
///
/// Mail from a sender without a name, or with a blank one, carries the bare
/// address. Nothing is checked here: the address is parsed when a `Mailer` or
/// an `Email` channel is built from the [`SmtpConfig`](crate::SmtpConfig)
/// that holds it.
///
/// ```
/// use abnegate_notify::Sender;
///
/// let sender = Sender::new("noreply@example.test").with_name("Notifications");
/// assert_eq!(sender.address, "noreply@example.test");
/// assert_eq!(sender.name.as_deref(), Some("Notifications"));
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Sender {
    /// The address every message is sent from.
    pub address: String,
    /// The display name shown beside `address`. `None` or a blank name sends
    /// from the bare address.
    pub name: Option<String>,
}

impl Sender {
    /// Send from `address`, with no name beside it.
    pub fn new(address: impl Into<String>) -> Self {
        Self {
            address: address.into(),
            name: None,
        }
    }

    /// Show `name` beside the address.
    #[must_use]
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Read a sender from a bare address or a `Name <address>` mailbox, as a
    /// `From` header or a settings file writes one.
    ///
    /// Text that is neither is refused with
    /// [`Error::Malformed`](crate::Error::Malformed), whose message never
    /// quotes it: an address is personal data, and the error is headed for a
    /// log line.
    ///
    /// ```
    /// use abnegate_notify::Sender;
    ///
    /// let sender = Sender::parse("Build Alerts <alerts@example.test>")?;
    /// assert_eq!(sender, Sender::new("alerts@example.test").with_name("Build Alerts"));
    ///
    /// let bare = Sender::parse("alerts@example.test")?;
    /// assert_eq!(bare, Sender::new("alerts@example.test"));
    ///
    /// assert!(Sender::parse("Build Alerts <alerts@example.test").is_err());
    /// # Ok::<(), abnegate_notify::Error>(())
    /// ```
    #[cfg(feature = "smtp")]
    #[cfg_attr(docsrs, doc(cfg(feature = "smtp")))]
    pub fn parse(text: &str) -> Result<Self, Error> {
        let parsed = parse_mailbox(text)?;
        Ok(Self {
            address: parsed.email.to_string(),
            name: parsed.name,
        })
    }

    #[cfg(feature = "smtp")]
    pub(crate) fn mailbox(&self) -> Result<Mailbox, Error> {
        let name = self.name.clone().filter(|name| !name.trim().is_empty());
        mailbox(&self.address, name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_sender_has_only_an_address() {
        let sender = Sender::new("noreply@example.test");
        assert_eq!(sender.address, "noreply@example.test");
        assert_eq!(sender.name, None);
    }

    #[test]
    fn with_name_sets_the_name_shown_beside_the_address() {
        let sender = Sender::new("noreply@example.test").with_name("Notifications");
        assert_eq!(sender.address, "noreply@example.test");
        assert_eq!(sender.name.as_deref(), Some("Notifications"));
    }

    #[cfg(feature = "smtp")]
    #[test]
    fn a_named_mailbox_parses_to_a_named_sender() {
        assert_eq!(
            Sender::parse("Build Alerts <alerts@example.test>").expect("parsed"),
            Sender::new("alerts@example.test").with_name("Build Alerts")
        );
        assert_eq!(
            Sender::parse("\"Ops, Night Shift\" <alerts@example.test>").expect("parsed"),
            Sender::new("alerts@example.test").with_name("Ops, Night Shift")
        );
    }

    #[cfg(feature = "smtp")]
    #[test]
    fn a_bare_address_parses_to_a_sender_without_a_name() {
        assert_eq!(
            Sender::parse("alerts@example.test").expect("parsed"),
            Sender::new("alerts@example.test")
        );
        assert_eq!(
            Sender::parse("<alerts@example.test>").expect("parsed"),
            Sender::new("alerts@example.test")
        );
    }

    #[cfg(feature = "smtp")]
    #[test]
    fn a_parsed_sender_sends_from_the_same_mailbox() {
        let sender = Sender::parse("Build Alerts <alerts@example.test>").expect("parsed");
        let from = sender.mailbox().expect("mailbox");
        assert_eq!(from.to_string(), "Build Alerts <alerts@example.test>");
    }

    #[cfg(feature = "smtp")]
    #[test]
    fn a_line_break_in_a_parsed_name_is_refused() {
        for text in [
            "Sam\r\nBcc: hidden@example.test <alerts@example.test>",
            "Sam\r\n Bcc: hidden@example.test <alerts@example.test>",
            "\"Sam\r\nBcc: hidden@example.test\" <alerts@example.test>",
            "Sam\nBcc: hidden@example.test <alerts@example.test>",
        ] {
            let error = Sender::parse(text).expect_err(text);
            assert!(matches!(error, Error::Malformed { .. }), "{text:?}");
        }
    }

    #[cfg(feature = "smtp")]
    #[test]
    fn an_unparseable_mailbox_is_refused_without_being_quoted() {
        for text in [
            "",
            "Build Alerts",
            "Build Alerts <private@example.test",
            "<>",
        ] {
            let error = Sender::parse(text).expect_err(text);
            assert!(matches!(error, Error::Malformed { .. }), "{text}");
            assert!(!error.to_string().contains("private"), "{error}");
        }
    }
}
