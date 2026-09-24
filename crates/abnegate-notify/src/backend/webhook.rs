//! The HTTP half that Slack and Discord share.

use std::time::Duration;

use abnegate_secret::sanitize;
use futures::StreamExt;
use reqwest::redirect::Policy;
use reqwest::{Client, Response, StatusCode};
use serde_json::Value;

use crate::endpoint::Endpoint;
use crate::error::Error;
use crate::fanout::{DEFAULT_TIMEOUT, MINIMUM_TIMEOUT};
use crate::text::truncate;

const USER_AGENT: &str = concat!("abnegate-notify/", env!("CARGO_PKG_VERSION"));
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_ERROR_BODY_BYTES: usize = 2_048;
const MAX_ERROR_BODY_CHARS: usize = 512;
const RETRY_AFTER: &str = "retry-after";

/// A JSON POST to one validated endpoint.
///
/// Redirects are refused rather than followed. Following one would let the
/// endpoint hand back a `Location` pointing at a private address and walk
/// straight around the host allowlist that [`Endpoint`] enforces, which is
/// the usual way an allowlisted webhook still turns into an SSRF.
///
/// Each request is bounded by [`DEFAULT_TIMEOUT`] until a backend sets its
/// own, and a backend that sets one hands the same budget to the fan-out.
#[derive(Debug)]
pub(crate) struct Webhook {
    endpoint: Endpoint,
    client: Client,
    timeout: Option<Duration>,
}

impl Webhook {
    pub(crate) fn new(endpoint: Endpoint) -> Result<Self, Error> {
        let client = Client::builder()
            .redirect(Policy::none())
            .connect_timeout(CONNECT_TIMEOUT)
            .user_agent(USER_AGENT)
            .build()
            .map_err(|error| Error::Malformed {
                message: strip_url(error),
            })?;

        Ok(Self {
            endpoint,
            client,
            timeout: None,
        })
    }

    pub(crate) fn host(&self) -> &str {
        self.endpoint.host()
    }

    /// The budget a backend set, which the fan-out honours in place of its own.
    pub(crate) fn timeout(&self) -> Option<Duration> {
        self.timeout
    }

    pub(crate) fn set_timeout(&mut self, timeout: Duration) {
        self.timeout = Some(timeout.max(MINIMUM_TIMEOUT));
    }

    pub(crate) async fn post(&self, payload: &Value) -> Result<Response, Error> {
        let timeout = self.timeout.unwrap_or(DEFAULT_TIMEOUT);
        let response = self
            .endpoint
            .post(&self.client)
            .timeout(timeout)
            .json(payload)
            .send()
            .await
            .map_err(|error| self.unsent(error, timeout))?;

        let status = response.status();
        if status.is_success() {
            return Ok(response);
        }

        if status == StatusCode::TOO_MANY_REQUESTS {
            return Err(Error::RateLimited {
                host: self.host().to_string(),
                retry_after: retry_after(&response),
            });
        }

        Err(Error::Rejected {
            host: self.host().to_string(),
            status: status.as_u16(),
            body: self.read_failure_body(response).await,
        })
    }

    fn unsent(&self, error: reqwest::Error, timeout: Duration) -> Error {
        if error.is_timeout() && !error.is_connect() {
            return Error::Timeout { after: timeout };
        }
        Error::Unreachable {
            host: self.host().to_string(),
            message: strip_url(error),
        }
    }

    /// The first of an error body, bounded before it is read.
    ///
    /// The body comes from whoever owns the endpoint, so it is neither
    /// trusted nor assumed to be small: reading it whole would let a hostile
    /// endpoint answer a webhook with an unbounded stream, and it reaches a
    /// log line, so it is sanitized like any other outside text.
    async fn read_failure_body(&self, response: Response) -> String {
        let mut stream = response.bytes_stream();
        let mut collected: Vec<u8> = Vec::new();

        while collected.len() < MAX_ERROR_BODY_BYTES {
            match stream.next().await {
                Some(Ok(chunk)) => collected.extend_from_slice(&chunk),
                Some(Err(_)) | None => break,
            }
        }
        collected.truncate(MAX_ERROR_BODY_BYTES);

        let text = String::from_utf8_lossy(&collected);
        let cleaned = truncate(sanitize(text.trim()).trim(), MAX_ERROR_BODY_CHARS);
        if cleaned.is_empty() {
            "no response body".to_string()
        } else {
            cleaned
        }
    }
}

/// A reqwest error's message with the request URL removed.
///
/// `reqwest::Error` appends `for url (...)` to its `Display`, and for a
/// webhook that URL is the credential, so it never survives into a message.
fn strip_url(error: reqwest::Error) -> String {
    error.without_url().to_string()
}

fn retry_after(response: &Response) -> Option<Duration> {
    let header = response.headers().get(RETRY_AFTER)?.to_str().ok()?;
    let seconds: f64 = header.trim().parse().ok()?;
    // Not from_secs_f64: it panics on the overflowing value a hostile header can carry.
    Duration::try_from_secs_f64(seconds).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    async fn webhook(server: &MockServer) -> Webhook {
        Webhook::new(Endpoint::for_test(&format!("{}/hook", server.uri()))).expect("client")
    }

    #[tokio::test]
    async fn a_success_returns_the_response() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/hook"))
            .respond_with(ResponseTemplate::new(204))
            .mount(&server)
            .await;

        let response = webhook(&server)
            .await
            .post(&json!({ "text": "hi" }))
            .await
            .expect("delivered");
        assert_eq!(response.status(), 204);
    }

    #[tokio::test]
    async fn a_rejection_carries_the_status_and_a_bounded_body() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(400).set_body_string("invalid_payload"))
            .mount(&server)
            .await;

        let error = webhook(&server)
            .await
            .post(&json!({}))
            .await
            .expect_err("rejected");

        match error {
            Error::Rejected { status, body, .. } => {
                assert_eq!(status, 400);
                assert_eq!(body, "invalid_payload");
            }
            other => panic!("expected a rejection, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn an_enormous_error_body_is_capped() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(500).set_body_string("x".repeat(200_000)))
            .mount(&server)
            .await;

        let error = webhook(&server)
            .await
            .post(&json!({}))
            .await
            .expect_err("rejected");

        match error {
            Error::Rejected { body, .. } => {
                assert!(
                    body.chars().count() <= MAX_ERROR_BODY_CHARS,
                    "body was {} chars",
                    body.chars().count()
                );
            }
            other => panic!("expected a rejection, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn an_error_body_is_sanitized_before_it_reaches_a_log() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(400).set_body_string(concat!(
                "\u{1b}]0;hijack\u{7}bad token ghp_",
                "0123456789abcdefghij"
            )))
            .mount(&server)
            .await;

        let error = webhook(&server)
            .await
            .post(&json!({}))
            .await
            .expect_err("rejected");

        let rendered = error.to_string();
        assert!(!rendered.contains('\u{1b}'), "control sequence survived");
        assert!(!rendered.contains(concat!("ghp_", "0123456789abcdefghij")));
        assert!(rendered.contains("[REDACTED]"));
    }

    #[tokio::test]
    async fn an_empty_error_body_says_so() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&server)
            .await;

        let error = webhook(&server)
            .await
            .post(&json!({}))
            .await
            .expect_err("rejected");
        assert!(error.to_string().contains("no response body"));
    }

    #[tokio::test]
    async fn a_rate_limit_reads_the_retry_after_header() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "3"))
            .mount(&server)
            .await;

        let error = webhook(&server)
            .await
            .post(&json!({}))
            .await
            .expect_err("rate limited");

        assert_eq!(error.retry_after(), Some(Duration::from_secs(3)));
        assert!(error.is_retryable());
    }

    #[tokio::test]
    async fn a_rate_limit_without_a_header_still_reports_itself() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(429))
            .mount(&server)
            .await;

        let error = webhook(&server)
            .await
            .post(&json!({}))
            .await
            .expect_err("rate limited");
        assert!(matches!(error, Error::RateLimited { .. }));
        assert_eq!(error.retry_after(), None);
    }

    #[tokio::test]
    async fn a_redirect_is_refused_rather_than_followed() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(302)
                    .insert_header("location", "http://169.254.169.254/latest/meta-data/"),
            )
            .mount(&server)
            .await;

        let error = webhook(&server)
            .await
            .post(&json!({}))
            .await
            .expect_err("a redirect is not a delivery");

        match error {
            Error::Rejected { status, .. } => assert_eq!(status, 302),
            other => panic!("expected the redirect to be reported, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn an_unreachable_host_never_names_the_url() {
        // Port 1 is privileged, so no concurrent test can bind it and answer.
        // Freeing an ephemeral port instead races every other test's mock
        // server, which then answers 200 and the assertion never runs.
        let webhook = Webhook::new(Endpoint::for_test(
            "http://127.0.0.1:1/services/T000/B000/xxxxSECRETxxxx",
        ))
        .expect("client");

        let error = webhook.post(&json!({})).await.expect_err("unreachable");
        let rendered = error.to_string();
        assert!(!rendered.contains("xxxxSECRETxxxx"), "leaked: {rendered}");
        assert!(!rendered.contains("/services/"), "leaked: {rendered}");
        assert!(!format!("{error:?}").contains("xxxxSECRETxxxx"));
    }

    #[tokio::test]
    async fn a_request_that_outlives_its_budget_is_a_timeout_not_an_outage() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(204).set_delay(Duration::from_secs(5)))
            .mount(&server)
            .await;

        let mut webhook = webhook(&server).await;
        webhook.set_timeout(Duration::from_millis(100));

        let error = webhook.post(&json!({})).await.expect_err("too slow");
        assert_eq!(
            error,
            Error::Timeout {
                after: Duration::from_millis(100)
            }
        );
    }

    #[test]
    fn a_zero_budget_is_raised_to_the_minimum() {
        let mut webhook =
            Webhook::new(Endpoint::for_test("http://127.0.0.1:1/hook")).expect("client");
        assert_eq!(webhook.timeout(), None);

        webhook.set_timeout(Duration::ZERO);
        assert_eq!(webhook.timeout(), Some(MINIMUM_TIMEOUT));
    }
}
