#[cfg(feature = "smtp")]
use lettre::message::Mailbox;

#[cfg(feature = "smtp")]
use crate::error::Error;
#[cfg(feature = "smtp")]
use crate::smtp::mailbox;

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
}
