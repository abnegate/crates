//! The HTTP half that Slack and Discord share.

mod authorization;

use std::time::Duration;

use abnegate_secret::SecretValue;
use abnegate_secret::conceal;
use abnegate_secret::sanitize;
use futures::StreamExt;
use reqwest::Client;
use reqwest::ClientBuilder;
use reqwest::RequestBuilder;
use reqwest::Response;
use reqwest::StatusCode;
use reqwest::header::CONTENT_TYPE;
use reqwest::redirect::Policy;
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::backend::webhook::authorization::Authorization;
use crate::endpoint::Endpoint;
use crate::error::Error;
use crate::error::MAXIMUM_ERROR_BODY_CHARACTERS;
use crate::fanout::DEFAULT_TIMEOUT;
use crate::fanout::MINIMUM_TIMEOUT;
use crate::text::truncate;

const USER_AGENT: &str = concat!("abnegate-notify/", env!("CARGO_PKG_VERSION"));
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const MAXIMUM_ERROR_BODY_BYTES: usize = 2_048;
const RETRY_AFTER: &str = "retry-after";
const JSON: &str = "application/json";
const EMPTY_FAILURE_BODY: &str = "no response body";
const CREDENTIAL_PART_LENGTH: usize = 12;

/// A POST to one validated [`Endpoint`], the client [`Slack`](crate::Slack)
/// and [`Discord`](crate::Discord) deliver through.
///
/// A third-party [`Notifier`](crate::Notifier) posts through it to keep the
/// same guarantees without restating them:
///
/// - Redirects are refused rather than followed. Following one would let the
///   endpoint hand back a `Location` pointing at a private address and walk
///   straight around the host allowlist that [`Endpoint`] enforces, which is
///   the usual way an allowlisted webhook still turns into an SSRF.
/// - No failure carries the URL, only its host, and a provider's error body
///   is read no further than its start and sanitized. The URL, each part of
///   its path or query long enough to be a token, and the credential from
///   [`Webhook::with_bearer`] or [`Webhook::with_basic`] are concealed from it
///   too, and never appear in `Debug`.
/// - A `429` becomes [`Error::RateLimited`] with the provider's `Retry-After`.
///
/// A payload goes out as JSON through [`Webhook::post`] or as a form through
/// [`Webhook::post_form`]. A provider that answers with something the caller
/// needs, such as the id of the message it created, is read through
/// [`Webhook::post_for`], [`Webhook::post_form_for`] or
/// [`Webhook::post_for_body`], none of which reads more than
/// [`Webhook::body_limit`] bytes of the answer.
///
/// Each request is bounded by [`DEFAULT_TIMEOUT`] until [`Webhook::with_timeout`]
/// sets another, and a notifier that sets one hands [`Webhook::timeout`] to the
/// fan-out through [`Notifier::timeout`](crate::Notifier::timeout).
///
/// ```no_run
/// # async fn example() -> Result<(), abnegate_notify::Error> {
/// use abnegate_notify::Endpoint;
/// use abnegate_notify::Webhook;
///
/// let endpoint = Endpoint::new("https://chat.example.com/hooks/xxxx", &["chat.example.com"])?;
/// let webhook = Webhook::new(endpoint)?;
/// webhook.post(&serde_json::json!({ "text": "deployed" })).await?;
/// # Ok(())
/// # }
/// ```
///
/// A provider that authenticates with a token and answers with the message it
/// created:
///
/// ```no_run
/// # async fn example() -> Result<(), abnegate_notify::Error> {
/// use abnegate_notify::Endpoint;
/// use abnegate_notify::Webhook;
///
/// #[derive(serde::Deserialize)]
/// struct Created {
///     id: String,
/// }
///
/// let endpoint = Endpoint::new("https://api.example.com/v1/messages", &["api.example.com"])?;
/// let webhook = Webhook::new(endpoint)?.with_bearer("token");
/// let created: Created = webhook.post_for(&serde_json::json!({ "text": "deployed" })).await?;
/// println!("sent {}", created.id);
/// # Ok(())
/// # }
/// ```
#[derive(Clone, Debug)]
pub struct Webhook {
    endpoint: Endpoint,
    client: Client,
    timeout: Option<Duration>,
    authorization: Option<Authorization>,
    body_limit: usize,
}

impl Webhook {
    /// The most bytes of a provider's answer that [`Webhook::post_for`] and
    /// its siblings read until [`Webhook::with_body_limit`] sets another cap.
    pub const DEFAULT_BODY_LIMIT: usize = 1024 * 1024;

    /// A client for `endpoint` that refuses redirects.
    pub fn new(endpoint: Endpoint) -> Result<Self, Error> {
        Self::built(endpoint, Client::builder())
    }

    fn built(endpoint: Endpoint, builder: ClientBuilder) -> Result<Self, Error> {
        let client = builder
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
            authorization: None,
            body_limit: Self::DEFAULT_BODY_LIMIT,
        })
    }

    /// A client for a server on the local machine, such as wiremock, for a
    /// caller's own tests. See [`Endpoint::for_test`] for what it accepts. It
    /// never goes through a proxy the environment names, so a test request
    /// and its credential stay on the machine.
    #[cfg(any(test, feature = "testing"))]
    #[cfg_attr(docsrs, doc(cfg(feature = "testing")))]
    pub fn for_test(url: &str) -> Result<Self, Error> {
        Self::built(Endpoint::for_test(url)?, Client::builder().no_proxy())
    }

    /// Give up on a request after `timeout` rather than [`DEFAULT_TIMEOUT`].
    ///
    /// A budget below the fan-out's minimum is raised to it.
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.set_timeout(timeout);
        self
    }

    /// Send `Authorization: Bearer <token>` with every request, replacing any
    /// credential set before.
    #[must_use]
    pub fn with_bearer(mut self, token: impl Into<SecretValue>) -> Self {
        self.authorization = Some(Authorization::Bearer(token.into()));
        self
    }

    /// Send HTTP basic authentication as `user` with `password` on every
    /// request, replacing any credential set before.
    #[must_use]
    pub fn with_basic(mut self, user: impl Into<String>, password: impl Into<SecretValue>) -> Self {
        self.authorization = Some(Authorization::basic(user.into(), password.into()));
        self
    }

    /// Read no more than `body_limit` bytes of a provider's answer, rather
    /// than [`Webhook::DEFAULT_BODY_LIMIT`].
    #[must_use]
    pub fn with_body_limit(mut self, body_limit: usize) -> Self {
        self.body_limit = body_limit;
        self
    }

    /// The endpoint this client posts to.
    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    /// The endpoint's host, which is safe to log.
    pub fn host(&self) -> &str {
        self.endpoint.host()
    }

    /// The budget [`Webhook::with_timeout`] set, if any, which the fan-out
    /// should honour in place of its own.
    pub fn timeout(&self) -> Option<Duration> {
        self.timeout
    }

    /// The most bytes of an answer this client reads.
    pub fn body_limit(&self) -> usize {
        self.body_limit
    }

    pub(crate) fn set_timeout(&mut self, timeout: Duration) {
        self.timeout = Some(timeout.max(MINIMUM_TIMEOUT));
    }

    /// POST `payload` as JSON, and succeed only on a `2xx` answer.
    ///
    /// A redirect is reported as [`Error::Rejected`], never followed. A
    /// payload with no JSON form is [`Error::Malformed`] and sends nothing.
    pub async fn post<T: Serialize + ?Sized + Sync>(&self, payload: &T) -> Result<(), Error> {
        self.send_json(payload).await.map(drop)
    }

    /// POST `payload` as JSON, and read the `2xx` answer as `R`.
    ///
    /// Fails as [`Webhook::post`] does, and with [`Error::Unreadable`] when
    /// the answer is larger than [`Webhook::body_limit`] or is not JSON of
    /// the shape `R` expects. That error follows an accepted request, so it
    /// is never retryable.
    pub async fn post_for<R: DeserializeOwned, T: Serialize + ?Sized + Sync>(
        &self,
        payload: &T,
    ) -> Result<R, Error> {
        let response = self.send_json(payload).await?;
        self.parse(response).await
    }

    /// POST `payload` as JSON, and return the `2xx` answer's raw bytes, no
    /// more than [`Webhook::body_limit`] of them.
    ///
    /// Fails as [`Webhook::post_for`] does, short of parsing.
    pub async fn post_for_body<T: Serialize + ?Sized + Sync>(
        &self,
        payload: &T,
    ) -> Result<Vec<u8>, Error> {
        let response = self.send_json(payload).await?;
        self.read_body(response).await
    }

    /// POST `form` as `application/x-www-form-urlencoded`, and succeed only
    /// on a `2xx` answer.
    ///
    /// `form` is a struct or a map of scalar values, or a sequence of pairs.
    /// Anything that cannot be encoded as a form is [`Error::Malformed`] and
    /// sends nothing.
    pub async fn post_form<T: Serialize + ?Sized + Sync>(&self, form: &T) -> Result<(), Error> {
        self.send_form(form).await.map(drop)
    }

    /// POST `form` as `application/x-www-form-urlencoded`, and read the
    /// `2xx` answer as JSON of the shape `R`.
    ///
    /// Fails as [`Webhook::post_form`] and [`Webhook::post_for`] do.
    pub async fn post_form_for<R: DeserializeOwned, T: Serialize + ?Sized + Sync>(
        &self,
        form: &T,
    ) -> Result<R, Error> {
        let response = self.send_form(form).await?;
        self.parse(response).await
    }

    async fn send_json<T: Serialize + ?Sized + Sync>(
        &self,
        payload: &T,
    ) -> Result<Response, Error> {
        let body = serde_json::to_vec(payload).map_err(Error::malformed)?;
        self.send(|request| request.header(CONTENT_TYPE, JSON).body(body))
            .await
    }

    async fn send_form<T: Serialize + ?Sized + Sync>(&self, form: &T) -> Result<Response, Error> {
        self.send(|request| request.form(form)).await
    }

    async fn send(
        &self,
        body: impl FnOnce(RequestBuilder) -> RequestBuilder,
    ) -> Result<Response, Error> {
        let timeout = self.timeout.unwrap_or(DEFAULT_TIMEOUT);
        let mut request = self.endpoint.post(&self.client).timeout(timeout);
        if let Some(authorization) = &self.authorization {
            request = authorization.apply(request)?;
        }

        let response = body(request)
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
        if error.is_builder() {
            return Error::malformed(self.conceal(&describe(error)));
        }
        if error.is_timeout() && !error.is_connect() {
            return Error::Timeout { after: timeout };
        }
        Error::Unreachable {
            host: self.host().to_string(),
            message: self.conceal(&strip_url(error)),
        }
    }

    async fn parse<R: DeserializeOwned>(&self, response: Response) -> Result<R, Error> {
        let body = self.read_body(response).await?;
        serde_json::from_slice(&body).map_err(|error| self.unreadable(&error.to_string()))
    }

    /// The whole of a successful answer, refused once it passes
    /// [`Webhook::body_limit`] rather than read on without end.
    async fn read_body(&self, response: Response) -> Result<Vec<u8>, Error> {
        let limit = u64::try_from(self.body_limit).unwrap_or(u64::MAX);
        if response
            .content_length()
            .is_some_and(|length| length > limit)
        {
            return Err(self.oversized());
        }

        let mut stream = response.bytes_stream();
        let mut collected: Vec<u8> = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|error| self.unreadable(&strip_url(error)))?;
            if collected.len() + chunk.len() > self.body_limit {
                return Err(self.oversized());
            }
            collected.extend_from_slice(&chunk);
        }
        Ok(collected)
    }

    /// The first of an error body, bounded before it is read.
    ///
    /// The body comes from whoever owns the endpoint, so it is neither
    /// trusted nor assumed to be small: reading it whole would let a hostile
    /// endpoint answer a webhook with an unbounded stream, and it reaches a
    /// log line, so it is sanitized like any other outside text. It is read
    /// past the bound by the longest secret this client holds and concealed
    /// before it is cut, so a secret straddling the bound is concealed whole
    /// rather than leaving its start behind.
    async fn read_failure_body(&self, response: Response) -> String {
        let limit = MAXIMUM_ERROR_BODY_BYTES + self.longest_secret();
        let mut stream = response.bytes_stream();
        let mut collected: Vec<u8> = Vec::new();

        while collected.len() < limit {
            match stream.next().await {
                Some(Ok(chunk)) => collected.extend_from_slice(&chunk),
                Some(Err(_)) | None => break,
            }
        }
        collected.truncate(limit);

        let concealed = self.conceal(&String::from_utf8_lossy(&collected));
        let text = cut_to_bytes(&concealed, MAXIMUM_ERROR_BODY_BYTES);
        let cleaned = truncate(sanitize(text.trim()).trim(), MAXIMUM_ERROR_BODY_CHARACTERS);
        if cleaned.is_empty() {
            EMPTY_FAILURE_BODY.to_string()
        } else {
            cleaned
        }
    }

    fn oversized(&self) -> Error {
        self.unreadable(&format!(
            "the answer is larger than the {} byte limit",
            self.body_limit
        ))
    }

    fn unreadable(&self, message: &str) -> Error {
        Error::unreadable(self.host(), self.conceal(message))
    }

    /// `text` without the URL or the credential this client sends, should a
    /// provider echo either back.
    fn conceal(&self, text: &str) -> String {
        self.secrets()
            .fold(text.to_string(), |text, secret| conceal(&text, secret))
    }

    fn longest_secret(&self) -> usize {
        self.secrets().map(str::len).max().unwrap_or(0)
    }

    fn secrets(&self) -> impl Iterator<Item = &str> {
        let url = self.endpoint.url().expose();
        std::iter::once(url).chain(credential_parts(url)).chain(
            self.authorization
                .iter()
                .flat_map(Authorization::secrets)
                .map(SecretValue::expose),
        )
    }
}

/// The parts of `url`'s path and query long enough to be a credential, such
/// as the token a Discord or Slack webhook URL ends in, which a provider may
/// echo on its own.
fn credential_parts(url: &str) -> impl Iterator<Item = &str> {
    let address = url.split_once("://").map_or(url, |(_, rest)| rest);
    let path = address.find('/').map_or("", |at| &address[at..]);
    path.split(['/', '?', '&', '=', '#'])
        .filter(|part| part.len() >= CREDENTIAL_PART_LENGTH)
}

/// `text` cut to at most `limit` bytes, on a character boundary.
fn cut_to_bytes(text: &str, limit: usize) -> &str {
    if text.len() <= limit {
        return text;
    }
    let end = (0..=limit)
        .rev()
        .find(|index| text.is_char_boundary(*index))
        .unwrap_or(0);
    &text[..end]
}

/// A reqwest error's message with the request URL removed.
///
/// `reqwest::Error` appends `for url (...)` to its `Display`, and for a
/// webhook that URL is the credential, so it never survives into a message.
fn strip_url(error: reqwest::Error) -> String {
    error.without_url().to_string()
}

/// A request that could not be built, with the cause reqwest keeps as its
/// source, since its own message only says that building failed.
fn describe(error: reqwest::Error) -> String {
    let cause = std::error::Error::source(&error).map(ToString::to_string);
    let message = strip_url(error);
    match cause {
        Some(cause) => format!("{message}: {cause}"),
        None => message,
    }
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
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::method;
    use wiremock::matchers::path;

    async fn webhook(server: &MockServer) -> Webhook {
        Webhook::for_test(&format!("{}/hook", server.uri())).expect("client")
    }

    #[tokio::test]
    async fn a_success_is_delivered() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/hook"))
            .respond_with(ResponseTemplate::new(204))
            .mount(&server)
            .await;

        webhook(&server)
            .await
            .post(&json!({ "text": "hi" }))
            .await
            .expect("delivered");
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
                    body.chars().count() <= MAXIMUM_ERROR_BODY_CHARACTERS,
                    "body was {} characters",
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
        let webhook = Webhook::for_test("http://127.0.0.1:1/services/T000/B000/xxxxSECRETxxxx")
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
        let mut webhook = Webhook::for_test("http://127.0.0.1:1/hook").expect("client");
        assert_eq!(webhook.timeout(), None);

        webhook.set_timeout(Duration::ZERO);
        assert_eq!(webhook.timeout(), Some(MINIMUM_TIMEOUT));
    }

    #[tokio::test]
    async fn a_credential_straddling_the_read_bound_is_concealed_whole() {
        let token = "~straddling-token-31";
        let filler = "\u{1f600}".repeat(MAXIMUM_ERROR_BODY_BYTES / 4 - 2);
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(401).set_body_string(format!("{filler}{token}")))
            .mount(&server)
            .await;

        let error = webhook(&server)
            .await
            .with_bearer(token)
            .post(&json!({}))
            .await
            .expect_err("unauthorized");

        match error {
            Error::Rejected { body, .. } => {
                assert!(body.starts_with(&filler), "the filler was kept");
                assert!(!body.contains('~'), "the token's start leaked: {body:?}");
            }
            other => panic!("expected a rejection, got {other:?}"),
        }
    }

    #[test]
    fn a_cut_never_splits_a_character() {
        assert_eq!(cut_to_bytes("a\u{e9}b", 2), "a");
        assert_eq!(cut_to_bytes("a\u{e9}b", 3), "a\u{e9}");
        assert_eq!(cut_to_bytes("abc", 8), "abc");
    }
}
