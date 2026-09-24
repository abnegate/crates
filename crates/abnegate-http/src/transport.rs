mod outbound;

use crate::body::read_capped;
use crate::client::HttpClient;
use crate::error::Result;
use crate::public::PublicClient;
use crate::response::HttpResponse;
use crate::transport::outbound::Outbound;
use async_trait::async_trait;
use reqwest::Client;
use reqwest::Method;
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(30);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// The default [`HttpClient`], backed by reqwest.
///
/// [`ReqwestHttpClient::new`] builds a client that trusts every URL it is
/// handed. To fetch caller-supplied URLs, convert a
/// [`PublicClient`](crate::PublicClient) with `From`: every request is then
/// checked before it is sent, and every name and redirect hop is checked as
/// it is resolved and followed.
///
/// Response bodies are read up to [`ReqwestHttpClient::DEFAULT_BODY_LIMIT`]
/// bytes unless [`ReqwestHttpClient::with_body_limit`] sets another cap, and
/// are decoded as UTF-8 with invalid sequences replaced.
#[derive(Debug, Clone)]
pub struct ReqwestHttpClient {
    outbound: Outbound,
    body_limit: usize,
}

impl ReqwestHttpClient {
    /// The largest response body read when no other cap is set: 8 MiB.
    pub const DEFAULT_BODY_LIMIT: usize = 8 * 1024 * 1024;

    /// Build a client with the default timeouts that trusts every URL.
    pub fn new() -> Result<Self> {
        let client = Client::builder()
            .timeout(TIMEOUT)
            .connect_timeout(CONNECT_TIMEOUT)
            .build()?;
        Ok(Self::from(client))
    }

    /// Refuse response bodies larger than `body_limit` bytes.
    pub fn with_body_limit(self, body_limit: usize) -> Self {
        Self { body_limit, ..self }
    }

    async fn send(
        &self,
        method: Method,
        url: &str,
        headers: Vec<(&str, String)>,
        body: Option<&str>,
    ) -> Result<HttpResponse> {
        let mut request = self.outbound.request(method, url)?;
        for (name, value) in headers {
            request = request.header(name, value);
        }
        if let Some(body) = body {
            request = request.body(body.to_string());
        }

        let response = self.outbound.execute(request.build()?).await?;
        let status = response.status().as_u16();
        let body = read_capped(response, self.body_limit).await?;
        let body = String::from_utf8(body)
            .unwrap_or_else(|error| String::from_utf8_lossy(error.as_bytes()).into_owned());
        Ok(HttpResponse { status, body })
    }
}

impl From<Client> for ReqwestHttpClient {
    fn from(client: Client) -> Self {
        Self {
            outbound: Outbound::Trusted(client),
            body_limit: Self::DEFAULT_BODY_LIMIT,
        }
    }
}

impl From<PublicClient> for ReqwestHttpClient {
    fn from(client: PublicClient) -> Self {
        Self {
            outbound: Outbound::Public(client),
            body_limit: Self::DEFAULT_BODY_LIMIT,
        }
    }
}

#[async_trait]
impl HttpClient for ReqwestHttpClient {
    async fn get(&self, url: &str, headers: Vec<(&str, String)>) -> Result<HttpResponse> {
        self.send(Method::GET, url, headers, None).await
    }

    async fn post(
        &self,
        url: &str,
        headers: Vec<(&str, String)>,
        body: &str,
    ) -> Result<HttpResponse> {
        self.send(Method::POST, url, headers, Some(body)).await
    }

    async fn put(
        &self,
        url: &str,
        headers: Vec<(&str, String)>,
        body: &str,
    ) -> Result<HttpResponse> {
        self.send(Method::PUT, url, headers, Some(body)).await
    }

    async fn patch(
        &self,
        url: &str,
        headers: Vec<(&str, String)>,
        body: &str,
    ) -> Result<HttpResponse> {
        self.send(Method::PATCH, url, headers, Some(body)).await
    }

    async fn delete(&self, url: &str, headers: Vec<(&str, String)>) -> Result<HttpResponse> {
        self.send(Method::DELETE, url, headers, None).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Error;
    use crate::public::public_client;
    use crate::test_support::LOOPBACK_SPELLINGS;
    use crate::test_support::assert_untouched;
    use crate::test_support::loopback_listener;
    use crate::test_support::serve_once;
    use std::sync::Arc;
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::body_string;
    use wiremock::matchers::header;
    use wiremock::matchers::method;
    use wiremock::matchers::path;

    fn client() -> ReqwestHttpClient {
        ReqwestHttpClient::new().expect("a client builds")
    }

    #[test]
    fn the_trait_stays_usable_behind_a_pointer() {
        let client: Arc<dyn HttpClient> = Arc::new(client());

        assert_eq!(Arc::strong_count(&client), 1);
    }

    #[tokio::test]
    async fn a_get_carries_its_headers_and_reads_the_body_back() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/thing"))
            .and(header("authorization", "Bearer token"))
            .respond_with(ResponseTemplate::new(200).set_body_string("read"))
            .mount(&server)
            .await;

        let response = client()
            .get(
                &format!("{}/thing", server.uri()),
                vec![("authorization", "Bearer token".to_string())],
            )
            .await
            .expect("the request reaches the server");

        assert_eq!(response.status, 200);
        assert_eq!(response.body, "read");
        assert!(response.is_success());
    }

    #[tokio::test]
    async fn every_body_carrying_method_sends_its_body() {
        for (verb, expected) in [("POST", 201), ("PUT", 200), ("PATCH", 200)] {
            let server = MockServer::start().await;
            Mock::given(method(verb))
                .and(path("/thing"))
                .and(body_string("written"))
                .respond_with(ResponseTemplate::new(expected))
                .mount(&server)
                .await;

            let client = client();
            let url = format!("{}/thing", server.uri());
            let response = match verb {
                "POST" => client.post(&url, Vec::new(), "written").await,
                "PUT" => client.put(&url, Vec::new(), "written").await,
                _ => client.patch(&url, Vec::new(), "written").await,
            }
            .expect("the request reaches the server");

            assert_eq!(response.status, expected, "{verb} answered wrongly");
        }
    }

    #[tokio::test]
    async fn a_delete_reports_the_status_it_was_given() {
        let server = MockServer::start().await;
        Mock::given(method("DELETE"))
            .and(path("/thing"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        let response = client()
            .delete(&format!("{}/thing", server.uri()), Vec::new())
            .await
            .expect("the request reaches the server");

        assert!(response.is_not_found());
    }

    #[tokio::test]
    async fn an_unreachable_host_is_a_transport_error() {
        let error = client()
            .get("http://127.0.0.1:1/", Vec::new())
            .await
            .expect_err("nothing listens there");

        assert!(matches!(error, Error::Request(_)), "{error}");
    }

    #[tokio::test]
    async fn a_transport_error_does_not_repeat_the_query_string() {
        let error = client()
            .get("http://127.0.0.1:1/search?key=hunter2", Vec::new())
            .await
            .expect_err("nothing listens there");

        assert!(!error.to_string().contains("hunter2"), "{error}");
        assert!(!format!("{error:?}").contains("hunter2"), "{error:?}");
    }

    #[tokio::test]
    async fn a_public_transport_refuses_every_spelling_of_loopback_before_connecting() {
        let client = ReqwestHttpClient::from(
            public_client(Duration::from_secs(5)).expect("a guarded client"),
        );

        for spelling in LOOPBACK_SPELLINGS {
            let (listener, port) = loopback_listener();
            let url = format!("http://{spelling}:{port}/");

            for response in [
                client.get(&url, Vec::new()).await,
                client.post(&url, Vec::new(), "body").await,
                client.put(&url, Vec::new(), "body").await,
                client.patch(&url, Vec::new(), "body").await,
                client.delete(&url, Vec::new()).await,
            ] {
                let error = response.expect_err("loopback must not be fetched");
                assert!(matches!(error, Error::PrivateAddress), "{url}: {error}");
            }
            assert_untouched(&listener);
        }
    }

    #[tokio::test]
    async fn a_body_cut_short_is_unreadable_rather_than_an_empty_success() {
        let port = serve_once(&b"HTTP/1.1 200 OK\r\ncontent-length: 100\r\n\r\nshort"[..]).await;

        let error = client()
            .get(&format!("http://127.0.0.1:{port}/?key=hunter2"), Vec::new())
            .await
            .expect_err("the body ended 95 bytes early");

        assert!(matches!(error, Error::UnreadableBody(_)), "{error}");
        assert!(!format!("{error:?}").contains("hunter2"), "{error:?}");
    }

    #[tokio::test]
    async fn a_huge_declared_length_under_an_unbounded_limit_is_unreadable_rather_than_a_panic() {
        let port = serve_once(format!(
            "HTTP/1.1 200 OK\r\ncontent-length: {}\r\n\r\ntiny",
            u64::MAX - 2
        ))
        .await;

        let error = client()
            .with_body_limit(usize::MAX)
            .get(&format!("http://127.0.0.1:{port}/"), Vec::new())
            .await
            .expect_err("the body ended four bytes in");

        assert!(matches!(error, Error::UnreadableBody(_)), "{error}");
    }

    #[tokio::test]
    async fn a_body_over_the_limit_is_refused() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string("b".repeat(64)))
            .mount(&server)
            .await;

        let error = client()
            .with_body_limit(16)
            .get(&server.uri(), Vec::new())
            .await
            .expect_err("64 bytes do not fit under 16");

        assert!(
            matches!(error, Error::OversizedBody { limit: 16 }),
            "{error}"
        );
    }

    #[tokio::test]
    async fn a_body_at_the_default_limit_is_read_whole() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_bytes(vec![b'a'; ReqwestHttpClient::DEFAULT_BODY_LIMIT]),
            )
            .mount(&server)
            .await;

        let response = client()
            .get(&server.uri(), Vec::new())
            .await
            .expect("the default limit is inclusive");

        assert_eq!(response.body.len(), ReqwestHttpClient::DEFAULT_BODY_LIMIT);
    }

    #[tokio::test]
    async fn a_body_that_is_not_utf8_is_decoded_with_replacements() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![b'o', 0xff, b'k']))
            .mount(&server)
            .await;

        let response = client()
            .get(&server.uri(), Vec::new())
            .await
            .expect("the request reaches the server");

        assert_eq!(response.body, "o\u{fffd}k");
    }
}
