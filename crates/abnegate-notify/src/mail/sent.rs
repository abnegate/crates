//! A message the mock was asked to send.

/// One message a [`MockMailer`](crate::MockMailer) was asked to send.
#[cfg_attr(docsrs, doc(cfg(feature = "testing")))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SentMail {
    pub recipient: String,
    pub subject: String,
    pub body: String,
}
