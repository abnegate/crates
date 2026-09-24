//! SearXNG client for server-side web search.
//!
//! Queries go to whatever URL is configured, so the instance may be a local
//! container, a private deployment, or a public one.

use std::time::Instant;

use reqwest::redirect::Policy;
use reqwest::{Client, Response};

use crate::config::WebSearchConfig;
use crate::entry::Entry;
use crate::error::Error;
use crate::hit::SearchHit;
use crate::observe::record;
use crate::outcome::Outcome;
use crate::query::{build_search_url, sanitize_query};
use crate::reply::Reply;
use crate::time_range::TimeRange;

const USER_AGENT: &str = concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"));

/// The most of an answer that is read. SearXNG's JSON for a page of results
/// is a few tens of kilobytes.
const MAXIMUM_BODY_BYTES: usize = 1024 * 1024;

/// HTTP client for one SearXNG instance.
///
/// Redirects are refused rather than followed, so a configured instance
/// cannot hand the request, and the query in it, to another host.
pub struct SearxngClient {
    http: Client,
    config: WebSearchConfig,
}

impl SearxngClient {
    /// A client for the instance `config` names.
    ///
    /// The result count and timeout are held to the same bounds
    /// [`WebSearchConfig::from_environment`] applies, however `config` was built.
    pub fn new(config: WebSearchConfig) -> Result<Self, Error> {
        let config = config.bounded();
        let http = Client::builder()
            .redirect(Policy::none())
            .timeout(config.timeout)
            .user_agent(USER_AGENT)
            .build()
            .map_err(Error::http)?;
        Ok(Self { http, config })
    }

    /// Query SearXNG and return at most `result_count` hits, optionally
    /// restricted to results published within `range`.
    ///
    /// While the config's `enabled` switch is off, this returns
    /// [`Error::Disabled`] and sends nothing.
    pub async fn search(
        &self,
        query: &str,
        range: Option<TimeRange>,
    ) -> Result<Vec<SearchHit>, Error> {
        let started = Instant::now();
        if !self.config.enabled {
            record(Outcome::Disabled, started.elapsed(), 0);
            return Err(Error::Disabled);
        }

        let query = sanitize_query(query);
        if query.is_empty() {
            record(Outcome::EmptyQuery, started.elapsed(), 0);
            return Ok(Vec::new());
        }

        let result = self.fetch(&query, range).await;
        match &result {
            Ok(hits) => record(Outcome::Succeeded, started.elapsed(), hits.len()),
            Err(error) => record(error.outcome(), started.elapsed(), 0),
        }
        result
    }

    async fn fetch(&self, query: &str, range: Option<TimeRange>) -> Result<Vec<SearchHit>, Error> {
        let url = build_search_url(&self.config.query_url, query, range);
        let response = self.http.get(&url).send().await.map_err(Error::http)?;
        let status = response.status();
        if !status.is_success() {
            return Err(Error::Status(status.as_u16()));
        }

        let body = read_bounded(response).await?;
        let reply: Reply =
            serde_json::from_slice(&body).map_err(|error| Error::malformed(&error))?;
        Ok(reply
            .results
            .into_iter()
            .filter_map(Entry::into_hit)
            .take(self.config.result_count)
            .collect())
    }
}

/// The body of `response`, refused once it passes [`MAXIMUM_BODY_BYTES`].
///
/// A declared length is checked up front, and the chunks are counted as they
/// arrive for an answer that declares none.
async fn read_bounded(mut response: Response) -> Result<Vec<u8>, Error> {
    let too_large = Error::TooLarge {
        limit: MAXIMUM_BODY_BYTES,
    };
    if response
        .content_length()
        .is_some_and(|length| length > MAXIMUM_BODY_BYTES as u64)
    {
        return Err(too_large);
    }

    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(Error::http)? {
        if body.len() + chunk.len() > MAXIMUM_BODY_BYTES {
            return Err(too_large);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::observe::observe_searches;
    use serde_json::json;
    use std::sync::{Mutex, PoisonError};
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use wiremock::matchers::{method, path, query_param, query_param_is_missing};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn test_client(query_url: String, result_count: usize) -> SearxngClient {
        SearxngClient::new(
            WebSearchConfig::new(query_url)
                .with_result_count(result_count)
                .with_timeout(Duration::from_secs(5)),
        )
        .expect("client")
    }

    #[tokio::test]
    async fn search_parses_searxng_json_and_respects_limit() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/search"))
            .and(query_param("q", "open source"))
            .and(query_param("format", "json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "results": [
                    {"title": "Example", "url": "https://example.com", "content": "A snippet"},
                    {"title": "Other", "url": "https://other.test", "content": ""},
                    {"title": "Skipped", "url": "https://skip.test", "content": "too many"}
                ]
            })))
            .mount(&server)
            .await;

        let client = test_client(format!("{}/search?q=<query>&format=json", server.uri()), 2);
        let hits = client.search("open source", None).await.expect("search");
        assert_eq!(
            hits,
            vec![
                SearchHit::new("Example", "https://example.com", "A snippet"),
                SearchHit::new("Other", "https://other.test", ""),
            ]
        );
    }

    #[tokio::test]
    async fn search_skips_results_without_urls() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/search"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "results": [
                    {"title": "No url", "content": "x"},
                    {"title": "", "url": "https://ok.test", "content": "ok"}
                ]
            })))
            .mount(&server)
            .await;

        let client = test_client(format!("{}/search?q=<query>&format=json", server.uri()), 5);
        let hits = client.search("q", None).await.expect("search");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title, "https://ok.test");
        assert_eq!(hits[0].url, "https://ok.test");
    }

    #[tokio::test]
    async fn search_returns_status_error_on_http_failure() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/search"))
            .respond_with(ResponseTemplate::new(429))
            .mount(&server)
            .await;

        let client = test_client(format!("{}/search?q=<query>&format=json", server.uri()), 5);
        let error = client.search("q", None).await.expect_err("http error");
        assert!(matches!(error, Error::Status(429)));
    }

    #[tokio::test]
    async fn search_returns_empty_for_blank_query() {
        let client = test_client(
            "http://127.0.0.1:1/search?q=<query>&format=json".to_string(),
            5,
        );
        assert!(client.search("   ", None).await.expect("empty").is_empty());
    }

    /// A schema the engine never receives is worthless, so the mock only
    /// answers a request that actually carries the narrowed range.
    #[tokio::test]
    async fn search_sends_the_range_to_the_engine() {
        for &range in TimeRange::ALL {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/search"))
                .and(query_param("q", "rust release"))
                .and(query_param(TimeRange::PARAMETER, range.as_str()))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "results": [{"title": "Rust", "url": "https://example.test", "content": "Fresh"}]
                })))
                .expect(1)
                .mount(&server)
                .await;

            let client = test_client(format!("{}/search?q=<query>&format=json", server.uri()), 5);
            let hits = client
                .search("rust release", Some(range))
                .await
                .unwrap_or_else(|error| panic!("{range} search failed: {error}"));

            assert_eq!(hits.len(), 1, "{range}");
        }
    }

    #[tokio::test]
    async fn search_without_a_range_sends_no_range_parameter() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/search"))
            .and(query_param_is_missing(TimeRange::PARAMETER))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "results": [{"title": "Rust", "url": "https://example.test", "content": ""}]
            })))
            .expect(1)
            .mount(&server)
            .await;

        let client = test_client(format!("{}/search?q=<query>&format=json", server.uri()), 5);
        assert_eq!(
            client
                .search("rust release", None)
                .await
                .expect("search")
                .len(),
            1
        );
    }

    fn search_url(server: &MockServer) -> String {
        format!("{}/search?q=<query>&format=json", server.uri())
    }

    fn results(count: usize) -> serde_json::Value {
        let results: Vec<serde_json::Value> = (0..count)
            .map(|index| json!({"title": format!("Result {index}"), "url": format!("https://example.test/{index}"), "content": ""}))
            .collect();
        json!({ "results": results })
    }

    #[tokio::test]
    async fn a_transport_failure_never_names_the_request_url() {
        let client = test_client(
            "http://127.0.0.1:1/search?token=hunter2&q=<query>&format=json".to_string(),
            5,
        );
        let error = client
            .search("my private question", None)
            .await
            .expect_err("nothing listens on port 1");

        assert!(matches!(error, Error::Http(_)), "{error:?}");
        let rendered = format!("{error} {error:?}");
        for leaked in ["hunter2", "private", "127.0.0.1:1/search"] {
            assert!(!rendered.contains(leaked), "leaked {leaked}: {rendered}");
        }
    }

    #[tokio::test]
    async fn a_redirect_is_refused_rather_than_followed() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/search"))
            .respond_with(
                ResponseTemplate::new(302).insert_header("location", "/elsewhere?format=json"),
            )
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/elsewhere"))
            .respond_with(ResponseTemplate::new(200).set_body_json(results(1)))
            .expect(0)
            .mount(&server)
            .await;

        let error = test_client(search_url(&server), 5)
            .search("rust", None)
            .await
            .expect_err("a redirect is not an answer");
        assert_eq!(error, Error::Status(302));
    }

    #[tokio::test]
    async fn an_oversized_answer_is_refused() {
        let server = MockServer::start().await;
        let padding = "x".repeat(MAXIMUM_BODY_BYTES);
        Mock::given(method("GET"))
            .and(path("/search"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({ "results": [], "padding": padding })),
            )
            .mount(&server)
            .await;

        let error = test_client(search_url(&server), 5)
            .search("rust", None)
            .await
            .expect_err("too large");
        assert_eq!(
            error,
            Error::TooLarge {
                limit: MAXIMUM_BODY_BYTES
            }
        );
    }

    #[tokio::test]
    async fn a_switched_off_client_refuses_without_sending_a_request() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_json(results(1)))
            .expect(0)
            .mount(&server)
            .await;

        let client =
            SearxngClient::new(WebSearchConfig::new(search_url(&server)).with_enabled(false))
                .expect("client");
        for query in ["rust", "   "] {
            assert_eq!(
                client.search(query, None).await,
                Err(Error::Disabled),
                "{query:?}"
            );
        }
        server.verify().await;
    }

    #[tokio::test]
    async fn a_switched_on_client_still_searches() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/search"))
            .respond_with(ResponseTemplate::new(200).set_body_json(results(1)))
            .expect(1)
            .mount(&server)
            .await;

        let client =
            SearxngClient::new(WebSearchConfig::new(search_url(&server)).with_enabled(true))
                .expect("client");
        assert_eq!(client.search("rust", None).await.expect("search").len(), 1);
        server.verify().await;
    }

    /// Chunked, so no length is declared and only counting the bytes as they
    /// arrive can stop it.
    #[tokio::test]
    async fn an_oversized_answer_of_undeclared_length_is_refused() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept");
            let mut request = [0u8; 4096];
            let _ = stream.read(&mut request).await;
            let chunk = vec![b' '; 64 * 1024];
            let _ = stream
                .write_all(b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ntransfer-encoding: chunked\r\n\r\n")
                .await;
            for _ in 0..(2 * MAXIMUM_BODY_BYTES / chunk.len()) {
                let header = format!("{:x}\r\n", chunk.len());
                if stream.write_all(header.as_bytes()).await.is_err()
                    || stream.write_all(&chunk).await.is_err()
                    || stream.write_all(b"\r\n").await.is_err()
                {
                    return;
                }
            }
            let _ = stream.write_all(b"0\r\n\r\n").await;
        });

        let error = test_client(format!("http://{address}/search?q=<query>&format=json"), 5)
            .search("rust", None)
            .await
            .expect_err("too large");
        assert_eq!(
            error,
            Error::TooLarge {
                limit: MAXIMUM_BODY_BYTES
            }
        );
    }

    #[tokio::test]
    async fn the_result_count_is_held_to_its_ceiling() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/search"))
            .respond_with(ResponseTemplate::new(200).set_body_json(results(30)))
            .mount(&server)
            .await;

        let hits = test_client(search_url(&server), 500)
            .search("rust", None)
            .await
            .expect("search");
        assert_eq!(hits.len(), 20);
    }

    /// A zero timeout would fail every request before it was sent.
    #[tokio::test]
    async fn a_zero_timeout_is_raised_to_the_floor() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/search"))
            .respond_with(ResponseTemplate::new(200).set_body_json(results(1)))
            .mount(&server)
            .await;

        let client = SearxngClient::new(
            WebSearchConfig::new(search_url(&server)).with_timeout(Duration::ZERO),
        )
        .expect("client");
        assert_eq!(client.search("rust", None).await.expect("search").len(), 1);
    }

    static OBSERVED: Mutex<Vec<Outcome>> = Mutex::new(Vec::new());

    fn observe(outcome: Outcome, _duration: Duration, _results: usize) {
        OBSERVED
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(outcome);
    }

    /// The only test that installs an observer, since the first one wins.
    #[tokio::test]
    async fn a_body_that_is_not_json_is_reported_as_malformed() {
        observe_searches(observe);

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/search"))
            .respond_with(ResponseTemplate::new(200).set_body_string("<html>blocked</html>"))
            .mount(&server)
            .await;

        let error = test_client(search_url(&server), 5)
            .search("rust", None)
            .await
            .expect_err("not JSON");
        assert!(matches!(error, Error::Malformed { .. }), "{error:?}");
        assert!(
            OBSERVED
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .contains(&Outcome::Malformed),
            "a decode failure was reported as something else"
        );
    }
}
