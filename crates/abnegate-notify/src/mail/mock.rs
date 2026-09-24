//! Mail that is recorded rather than sent.

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::Mutex;

use crate::error::Error;
use crate::mail::Mail;
use crate::mail::sent::SentMail;

/// A [`Mail`] that records instead of sending.
///
/// It takes and answers exactly what the real one does, so a caller under
/// test can hold either and assert on what would have gone out. It needs only
/// the `testing` feature, so a downstream test suite need not build `lettre`.
#[cfg_attr(docsrs, doc(cfg(feature = "testing")))]
#[derive(Clone, Debug, Default)]
pub struct MockMailer {
    sent: Arc<Mutex<Vec<SentMail>>>,
}

impl MockMailer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Everything sent so far, oldest first.
    pub async fn sent(&self) -> Vec<SentMail> {
        self.sent.lock().await.clone()
    }

    pub async fn clear(&self) {
        self.sent.lock().await.clear();
    }
}

#[async_trait]
impl Mail for MockMailer {
    async fn send(&self, recipient: &str, subject: &str, body: &str) -> Result<(), Error> {
        self.sent.lock().await.push(SentMail {
            recipient: recipient.to_string(),
            subject: subject.to_string(),
            body: body.to_string(),
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[tokio::test]
    async fn the_mock_stands_in_wherever_mail_is_expected() {
        let mailer: Arc<dyn Mail> = Arc::new(MockMailer::new());
        mailer
            .send("person@example.test", "Subject", "Body")
            .await
            .expect("recorded");
    }
}
