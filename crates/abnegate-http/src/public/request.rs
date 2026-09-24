use crate::error::Error;
use crate::error::Result;
use crate::public::client::PublicClient;
use reqwest::Body;
use reqwest::Request;
use reqwest::RequestBuilder;
use reqwest::Response;
use reqwest::Version;
use reqwest::header::HeaderMap;
use reqwest::header::HeaderName;
use reqwest::header::HeaderValue;
use reqwest::multipart::Form;
use serde::Serialize;
use std::fmt::Display;
use std::time::Duration;

/// A request a [`PublicClient`] started after checking its URL.
///
/// It shapes a request the way reqwest's `RequestBuilder` does, but never
/// hands back the client underneath: that client connects to an IP literal
/// without consulting its resolver, so anything sent through it directly
/// would skip the check. [`PublicRequest::send`] checks the URL again on the
/// way out, and the [`Request`] that [`PublicRequest::build`] returns goes
/// through the guard only when [`PublicClient::execute`] sends it.
///
/// ```
/// use abnegate_http::{Error, public_client};
/// use std::time::Duration;
///
/// let client = public_client(Duration::from_secs(10))?;
/// let request = client
///     .post("https://example.com/submit")?
///     .bearer_auth("token")
///     .json(&["payload"])
///     .build()?;
///
/// assert_eq!(request.url().as_str(), "https://example.com/submit");
/// # Ok::<(), Error>(())
/// ```
///
/// There is no way back to the client underneath:
///
/// ```compile_fail,E0599
/// use abnegate_http::public_client;
/// use std::time::Duration;
///
/// let client = public_client(Duration::from_secs(10))?;
/// let (unguarded, _) = client.get("https://example.com/")?.build_split();
/// # let _: reqwest::Client = unguarded;
/// # Ok::<(), abnegate_http::Error>(())
/// ```
#[derive(Debug)]
#[must_use = "a request does nothing until it is sent"]
pub struct PublicRequest {
    builder: Result<RequestBuilder>,
}

impl PublicRequest {
    pub(super) fn new(builder: RequestBuilder) -> Self {
        Self {
            builder: Ok(builder),
        }
    }

    /// Add a header, failing the request if the name or value does not parse.
    pub fn header<N, V>(self, name: N, value: V) -> Self
    where
        HeaderName: TryFrom<N>,
        HeaderValue: TryFrom<V>,
    {
        self.and_then(|builder| {
            let name = HeaderName::try_from(name).map_err(|_| Error::InvalidHeaderName)?;
            let value = HeaderValue::try_from(value).map_err(|_| Error::InvalidHeaderValue)?;
            Ok(builder.header::<HeaderName, HeaderValue>(name, value))
        })
    }

    /// Merge `headers` into the ones already set.
    pub fn headers(self, headers: HeaderMap) -> Self {
        self.map(|builder| builder.headers(headers))
    }

    /// Append `query` to the URL's query string.
    pub fn query<T: Serialize + ?Sized>(self, query: &T) -> Self {
        self.map(|builder| builder.query(query))
    }

    /// Send `json` as the body, with a JSON content type unless one is set.
    pub fn json<T: Serialize + ?Sized>(self, json: &T) -> Self {
        self.map(|builder| builder.json(json))
    }

    /// Send `form` URL-encoded as the body, with a form content type unless
    /// one is set.
    pub fn form<T: Serialize + ?Sized>(self, form: &T) -> Self {
        self.map(|builder| builder.form(form))
    }

    /// Send `form` as a `multipart/form-data` body.
    pub fn multipart(self, form: Form) -> Self {
        self.map(|builder| builder.multipart(form))
    }

    /// Set the body.
    pub fn body<T: Into<Body>>(self, body: T) -> Self {
        self.map(|builder| builder.body(body))
    }

    /// Authenticate with a bearer `token`, marked sensitive.
    pub fn bearer_auth<T: Display>(self, token: T) -> Self {
        self.map(|builder| builder.bearer_auth(token))
    }

    /// Authenticate with HTTP basic authentication, marked sensitive.
    pub fn basic_auth<U: Display, P: Display>(self, username: U, password: Option<P>) -> Self {
        self.map(|builder| builder.basic_auth(username, password))
    }

    /// Bound this request by `timeout` in place of the client's.
    pub fn timeout(self, timeout: Duration) -> Self {
        self.map(|builder| builder.timeout(timeout))
    }

    /// Send the request over HTTP `version`.
    pub fn version(self, version: Version) -> Self {
        self.map(|builder| builder.version(version))
    }

    /// Copy the request, unless its body is a stream or it has already failed.
    pub fn try_clone(&self) -> Option<Self> {
        let builder = self.builder.as_ref().ok()?.try_clone()?;
        Some(Self::new(builder))
    }

    /// Build the request without sending it.
    ///
    /// Only [`PublicClient::execute`] sends the result through the guard.
    pub fn build(self) -> Result<Request> {
        Ok(self.builder?.build()?)
    }

    /// Send the request, refusing its URL again first if it must not be
    /// fetched.
    pub async fn send(self) -> Result<Response> {
        let (client, request) = self.builder?.build_split();
        PublicClient::new(client).execute(request?).await
    }

    fn map(self, apply: impl FnOnce(RequestBuilder) -> RequestBuilder) -> Self {
        Self {
            builder: self.builder.map(apply),
        }
    }

    fn and_then(self, apply: impl FnOnce(RequestBuilder) -> Result<RequestBuilder>) -> Self {
        Self {
            builder: self.builder.and_then(apply),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::public::public_client;
    use crate::test_support::LOOPBACK_SPELLINGS;
    use crate::test_support::assert_untouched;
    use crate::test_support::loopback_listener;
    use reqwest::Method;
    use reqwest::Url;
    use reqwest::header::AUTHORIZATION;
    use reqwest::header::CONTENT_TYPE;

    const TIMEOUT: Duration = Duration::from_secs(5);
    const PUBLIC: &str = "https://example.com/submit";

    fn client() -> PublicClient {
        public_client(TIMEOUT).expect("a guarded client")
    }

    fn start(client: &PublicClient) -> PublicRequest {
        client.post(PUBLIC).expect("a public URL is fetchable")
    }

    fn every_shape(client: &PublicClient) -> Vec<(&'static str, PublicRequest)> {
        vec![
            ("header", start(client).header("x-probe", "1")),
            ("headers", start(client).headers(HeaderMap::new())),
            ("query", start(client).query(&[("page", "2")])),
            ("json", start(client).json(&["probe"])),
            ("form", start(client).form(&[("probe", "1")])),
            (
                "multipart",
                start(client).multipart(Form::new().text("probe", "1")),
            ),
            ("body", start(client).body("probe")),
            ("bearer_auth", start(client).bearer_auth("token")),
            (
                "basic_auth",
                start(client).basic_auth("user", Some("secret")),
            ),
            ("timeout", start(client).timeout(TIMEOUT)),
            ("version", start(client).version(Version::HTTP_11)),
        ]
    }

    /// Point a started request at `url` behind the check that started it, as
    /// a method that rewrote the URL would.
    fn retarget(request: PublicRequest, url: &Url) -> PublicRequest {
        let (client, request) = request.builder.expect("the request started").build_split();
        let mut request = request.expect("the request builds");
        *request.url_mut() = url.clone();
        PublicRequest::new(RequestBuilder::from_parts(client, request))
    }

    fn loopback(spelling: &str, port: u16) -> Url {
        Url::parse(&format!("http://{spelling}:{port}/")).expect("a URL")
    }

    #[tokio::test]
    async fn every_shape_built_then_aimed_at_loopback_is_refused_before_a_connection_is_made() {
        let client = client();

        for spelling in LOOPBACK_SPELLINGS {
            let (listener, port) = loopback_listener();
            let url = loopback(spelling, port);

            for (shape, request) in every_shape(&client) {
                let mut request = request.build().expect("the request builds");
                *request.url_mut() = url.clone();

                let error = client
                    .execute(request)
                    .await
                    .expect_err("loopback must not be fetched");
                assert!(
                    matches!(error, Error::PrivateAddress),
                    "{shape} {url}: {error}"
                );
            }
            assert_untouched(&listener);
        }
    }

    #[tokio::test]
    async fn every_shape_sent_to_loopback_is_refused_before_a_connection_is_made() {
        let client = client();

        for spelling in LOOPBACK_SPELLINGS {
            let (listener, port) = loopback_listener();
            let url = loopback(spelling, port);

            for (shape, request) in every_shape(&client) {
                let error = retarget(request, &url)
                    .send()
                    .await
                    .expect_err("loopback must not be fetched");
                assert!(
                    matches!(error, Error::PrivateAddress),
                    "{shape} {url}: {error}"
                );
            }
            assert_untouched(&listener);
        }
    }

    #[test]
    fn every_method_shapes_the_request_it_names() {
        let mut merged = HeaderMap::new();
        merged.insert("x-merged", HeaderValue::from_static("yes"));

        let request = start(&client())
            .header("x-probe", "1")
            .headers(merged)
            .query(&[("page", "2")])
            .bearer_auth("token")
            .timeout(TIMEOUT)
            .version(Version::HTTP_11)
            .json(&["probe"])
            .build()
            .expect("the request builds");

        assert_eq!(request.method(), Method::POST);
        assert_eq!(request.url().as_str(), "https://example.com/submit?page=2");
        assert_eq!(request.headers()["x-probe"], "1");
        assert_eq!(request.headers()["x-merged"], "yes");
        assert_eq!(request.headers()[AUTHORIZATION], "Bearer token");
        assert!(request.headers()[AUTHORIZATION].is_sensitive());
        assert_eq!(request.headers()[CONTENT_TYPE], "application/json");
        assert_eq!(
            request.body().and_then(Body::as_bytes),
            Some(&b"[\"probe\"]"[..])
        );
        assert_eq!(request.timeout(), Some(&TIMEOUT));
        assert_eq!(request.version(), Version::HTTP_11);
    }

    #[test]
    fn every_body_method_sets_its_body_and_content_type() {
        let client = client();

        let form = start(&client)
            .form(&[("probe", "1")])
            .build()
            .expect("the request builds");
        assert_eq!(
            form.headers()[CONTENT_TYPE],
            "application/x-www-form-urlencoded"
        );
        assert_eq!(form.body().and_then(Body::as_bytes), Some(&b"probe=1"[..]));

        let raw = start(&client)
            .body("probe")
            .build()
            .expect("the request builds");
        assert_eq!(raw.body().and_then(Body::as_bytes), Some(&b"probe"[..]));

        let multipart = start(&client)
            .multipart(Form::new().text("probe", "1"))
            .build()
            .expect("the request builds");
        assert!(
            multipart.headers()[CONTENT_TYPE]
                .to_str()
                .is_ok_and(|value| value.starts_with("multipart/form-data; boundary=")),
            "{:?}",
            multipart.headers()[CONTENT_TYPE]
        );
    }

    #[test]
    fn basic_authentication_is_encoded_and_marked_sensitive() {
        let request = start(&client())
            .basic_auth("user", Some("secret"))
            .build()
            .expect("the request builds");

        assert_eq!(request.headers()[AUTHORIZATION], "Basic dXNlcjpzZWNyZXQ=");
        assert!(request.headers()[AUTHORIZATION].is_sensitive());
    }

    #[tokio::test]
    async fn a_header_that_does_not_parse_fails_the_request() {
        let client = client();

        let invalid_name = start(&client).header("line\nbreak", "value");
        assert!(
            invalid_name.try_clone().is_none(),
            "a failed request cloned"
        );
        let error = invalid_name
            .json(&["a later method keeps the first failure"])
            .send()
            .await
            .expect_err("the name does not parse");
        assert!(matches!(error, Error::InvalidHeaderName), "{error}");

        let error = start(&client)
            .header("x-probe", "line\nbreak")
            .build()
            .expect_err("the value does not parse");
        assert!(matches!(error, Error::InvalidHeaderValue), "{error}");
    }

    #[test]
    fn a_clone_carries_the_same_request_unless_its_body_is_a_stream() {
        let client = client();
        let request = start(&client).header("x-probe", "1").body("probe");

        let clone = request
            .try_clone()
            .expect("a byte body clones")
            .build()
            .expect("the clone builds");
        let original = request.build().expect("the original builds");

        assert_eq!(clone.url(), original.url());
        assert_eq!(clone.headers(), original.headers());
        assert_eq!(
            clone.body().and_then(Body::as_bytes),
            original.body().and_then(Body::as_bytes)
        );
        assert!(
            start(&client)
                .multipart(Form::new().text("probe", "1"))
                .try_clone()
                .is_none(),
            "a streamed body cloned"
        );
    }
}
