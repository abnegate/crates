//! A mail sender of the caller's own, failing the way a relay does.

use abnegate_notify::{Error, Mail};
use async_trait::async_trait;

/// A sender whose relay is turning messages away for now.
struct Deferring;

#[async_trait]
impl Mail for Deferring {
    async fn send(&self, _recipient: &str, _subject: &str, _body: &str) -> Result<(), Error> {
        Err(Error::smtp(
            "mail.example.test",
            "421 4.7.0 try again later",
        ))
    }
}

#[tokio::test]
async fn a_sender_of_the_callers_own_reports_a_relay_failure() {
    let error = Deferring
        .send("person@example.test", "Subject", "Body")
        .await
        .expect_err("the relay deferred the message");

    assert_eq!(
        error.to_string(),
        "SMTP delivery via mail.example.test failed: 421 4.7.0 try again later"
    );
    assert!(error.is_retryable());
}
