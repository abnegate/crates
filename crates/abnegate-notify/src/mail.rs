//! One arbitrary message to one recipient.

use async_trait::async_trait;

use crate::error::Error;

#[cfg(feature = "smtp")]
mod mailer;
#[cfg(feature = "mock")]
mod mock;
#[cfg(feature = "mock")]
mod sent;

#[cfg(feature = "smtp")]
pub use crate::mail::mailer::Mailer;
#[cfg(feature = "mock")]
pub use crate::mail::mock::MockMailer;
#[cfg(feature = "mock")]
pub use crate::mail::sent::SentMail;

/// Sends a subject and a body to one recipient.
///
/// This is the transactional half of the crate, with no
/// [`Notification`](crate::Notification) involved. The `smtp` feature provides
/// `Mailer`, which sends through a relay, and the `mock` feature provides
/// `MockMailer`, which records instead, so a caller can hold either behind
/// this trait and assert on what would have gone out.
///
/// Unlike a notification, the text is sent exactly as it was given. Redaction
/// would eat the one-time token in an account-flow link, which is the whole
/// payload of the message it appears in.
#[async_trait]
pub trait Mail: Send + Sync {
    /// Send `subject` and `body` to `recipient`.
    async fn send(&self, recipient: &str, subject: &str, body: &str) -> Result<(), Error>;
}
