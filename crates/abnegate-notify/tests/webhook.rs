//! What a third-party notifier can do with an `Endpoint` and a `Webhook`.

use std::collections::BTreeMap;
use std::time::Duration;

use abnegate_notify::Endpoint;
use abnegate_notify::Error;
use abnegate_notify::Webhook;
use serde_json::json;

const ALLOWED: &[&str] = &["hooks.example.invalid"];
const SECRET: &str = "xxxxSECRETxxxx";

fn endpoint() -> Endpoint {
    Endpoint::new(
        &format!("https://HOOKS.Example.INVALID/services/T000/{SECRET}"),
        ALLOWED,
    )
    .expect("allowed host")
}

#[test]
fn an_endpoint_reads_back_the_url_it_checked() {
    let endpoint = endpoint();

    assert_eq!(
        endpoint.url().expose(),
        format!("https://hooks.example.invalid/services/T000/{SECRET}")
    );
    assert_eq!(endpoint.host(), "hooks.example.invalid");
}

#[test]
fn the_read_back_url_stays_redacted_when_formatted() {
    let endpoint = endpoint();

    assert!(!format!("{:?}", endpoint.url()).contains(SECRET));
    assert!(!endpoint.url().to_string().contains(SECRET));
}

#[test]
fn a_webhook_reports_its_endpoint_host_and_budget() {
    let webhook = Webhook::new(endpoint()).expect("client");
    assert_eq!(webhook.host(), "hooks.example.invalid");
    assert_eq!(webhook.endpoint().host(), "hooks.example.invalid");
    assert_eq!(webhook.timeout(), None);

    let bounded = webhook.with_timeout(Duration::from_secs(2));
    assert_eq!(bounded.timeout(), Some(Duration::from_secs(2)));
}

#[test]
fn a_zero_budget_is_raised_above_zero() {
    let webhook = Webhook::new(endpoint())
        .expect("client")
        .with_timeout(Duration::ZERO);
    assert!(
        webhook
            .timeout()
            .is_some_and(|timeout| timeout > Duration::ZERO)
    );
}

#[tokio::test]
async fn a_failed_post_names_the_host_and_never_the_url() {
    let webhook = Webhook::new(endpoint())
        .expect("client")
        .with_timeout(Duration::from_secs(5));

    let error = webhook
        .post(&json!({ "text": "hello" }))
        .await
        .expect_err("an .invalid host never resolves");

    assert!(
        matches!(error, Error::Unreachable { .. } | Error::Timeout { .. }),
        "unexpected {error:?}"
    );
    let rendered = format!("{error} {error:?}");
    assert!(!rendered.contains(SECRET), "leaked: {rendered}");
    assert!(!rendered.contains("/services/"), "leaked: {rendered}");
}

/// A payload that cannot be serialized was reported by the transport as
/// unreachable, and so as worth retrying, when sending it again unchanged can
/// never succeed.
#[tokio::test]
async fn a_payload_that_cannot_be_serialized_is_malformed_and_never_retried() {
    let webhook = Webhook::new(endpoint()).expect("client");
    let payload = BTreeMap::from([((1_u8, 2_u8), "a key JSON cannot hold")]);

    let error = webhook
        .post(&payload)
        .await
        .expect_err("a tuple key has no JSON form");

    assert!(
        matches!(error, Error::Malformed { .. }),
        "unexpected {error:?}"
    );
    assert!(!error.is_retryable());
}
