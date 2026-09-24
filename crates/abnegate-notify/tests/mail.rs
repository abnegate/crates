//! The mock mailer, used the way a caller's own test suite would use it.

#![cfg(feature = "testing")]

use std::sync::Arc;

use abnegate_notify::{Mail, MockMailer, SentMail};

#[tokio::test]
async fn a_caller_asserts_on_exactly_what_would_have_gone_out() {
    let mailer = MockMailer::new();
    let held: Arc<dyn Mail> = Arc::new(mailer.clone());

    held.send(
        "person@example.test",
        "Verify your email address",
        "Open https://example.test/verify?t=abc",
    )
    .await
    .expect("recorded");

    assert_eq!(
        mailer.sent().await,
        vec![SentMail::new(
            "person@example.test",
            "Verify your email address",
            "Open https://example.test/verify?t=abc",
        )]
    );
}
