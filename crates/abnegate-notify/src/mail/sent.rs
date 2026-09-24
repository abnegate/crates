//! A message the mock was asked to send.

/// One message a [`MockMailer`](crate::MockMailer) was asked to send.
#[cfg_attr(docsrs, doc(cfg(feature = "testing")))]
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct SentMail {
    /// The address it was sent to, exactly as given.
    pub recipient: String,
    /// The subject line, exactly as given.
    pub subject: String,
    /// The body, exactly as given.
    pub body: String,
}

impl SentMail {
    /// The message a test expects to find in
    /// [`MockMailer::sent`](crate::MockMailer::sent).
    pub fn new(
        recipient: impl Into<String>,
        subject: impl Into<String>,
        body: impl Into<String>,
    ) -> Self {
        Self {
            recipient: recipient.into(),
            subject: subject.into(),
            body: body.into(),
        }
    }
}
