use std::time::Duration;

use abnegate_secret::REDACTED;
use abnegate_secret::SecretValue;
use serde_json::Value;
use serde_json::json;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;
use wiremock::matchers::header;
use wiremock::matchers::method;
use wiremock::matchers::path;

use super::*;

const MODEL: &str = "bge-reranker-v2-m3";
const RERANK: &str = "/v1/rerank";
const KEY: &str = "sk-litellm-master-0123456789";
const SHAPELESS_KEY: &str = "gateway-hunter-2024";
const QUERY: &str = "What does should_skip_blob do?";

fn documents(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_string()).collect()
}

fn url(server: &MockServer) -> String {
    format!("{}{RERANK}", server.uri())
}

fn provider(config: HttpRerankConfig) -> HttpRerankProvider {
    HttpRerankProvider::new(config).unwrap()
}

fn answer(results: Value) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "id": "rerank-0",
        "model": MODEL,
        "results": results,
        "usage": { "total_tokens": 12 }
    }))
}

fn hit(index: usize, relevance_score: f32) -> Value {
    json!({ "index": index, "relevance_score": relevance_score })
}

async fn serve(server: &MockServer, response: ResponseTemplate) {
    Mock::given(method("POST"))
        .and(path(RERANK))
        .respond_with(response)
        .mount(server)
        .await;
}

async fn sent_body(server: &MockServer) -> Value {
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1, "exactly one request is sent");
    serde_json::from_slice(&requests[0].body).unwrap()
}

async fn scores_for(
    response: ResponseTemplate,
    values: &[&str],
) -> Result<Vec<f32>, ProviderError> {
    let server = MockServer::start().await;
    serve(&server, response).await;
    provider(HttpRerankConfig::new(url(&server), MODEL))
        .rerank(QUERY, &documents(values))
        .await
}

async fn refusal(status: u16, body: &str) -> ProviderError {
    let server = MockServer::start().await;
    serve(
        &server,
        ResponseTemplate::new(status).set_body_raw(body.to_string(), "application/json"),
    )
    .await;
    provider(HttpRerankConfig::new(url(&server), MODEL).with_api_key(KEY))
        .rerank(QUERY, &documents(&["fn should_skip_blob()"]))
        .await
        .unwrap_err()
}

async fn answered_with_shapeless_key(response: ResponseTemplate) -> ProviderError {
    let server = MockServer::start().await;
    serve(&server, response).await;
    provider(HttpRerankConfig::new(url(&server), MODEL).with_api_key(SHAPELESS_KEY))
        .rerank(QUERY, &documents(&["fn should_skip_blob()"]))
        .await
        .unwrap_err()
}

fn is_parse_error_saying(error: &ProviderError, expected: &str) -> bool {
    matches!(error, ProviderError::Parse { detail } if detail == expected)
}

#[tokio::test]
async fn the_body_carries_the_model_the_query_and_every_document() {
    let server = MockServer::start().await;
    serve(&server, answer(json!([hit(0, 0.9), hit(1, 0.1)]))).await;

    provider(HttpRerankConfig::new(url(&server), MODEL))
        .rerank(QUERY, &documents(&["first", "second"]))
        .await
        .unwrap();

    assert_eq!(
        sent_body(&server).await,
        json!({ "model": MODEL, "query": QUERY, "documents": ["first", "second"] })
    );
}

#[tokio::test]
async fn the_url_is_posted_to_as_given() {
    let server = MockServer::start().await;
    serve(&server, answer(json!([hit(0, 0.9)]))).await;

    provider(HttpRerankConfig::new(url(&server), MODEL))
        .rerank(QUERY, &documents(&["first"]))
        .await
        .unwrap();

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests[0].method.as_str(), "POST");
    assert_eq!(requests[0].url.path(), RERANK);
}

#[tokio::test]
async fn a_trailing_slash_on_the_url_is_dropped() {
    let server = MockServer::start().await;
    serve(&server, answer(json!([hit(0, 0.9)]))).await;

    provider(HttpRerankConfig::new(format!("{}/", url(&server)), MODEL))
        .rerank(QUERY, &documents(&["first"]))
        .await
        .unwrap();

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests[0].url.path(), RERANK);
}

#[tokio::test]
async fn no_authorization_is_sent_without_a_key() {
    for config in [
        HttpRerankConfig::new("", MODEL),
        HttpRerankConfig::new("", MODEL).with_api_key(""),
        HttpRerankConfig::new("", MODEL).with_api_key(" \t\n "),
    ] {
        let server = MockServer::start().await;
        serve(&server, answer(json!([hit(0, 0.9)]))).await;
        let config = HttpRerankConfig {
            url: url(&server),
            ..config
        };

        provider(config)
            .rerank(QUERY, &documents(&["first"]))
            .await
            .unwrap();

        let requests = server.received_requests().await.unwrap();
        assert!(requests[0].headers.get("authorization").is_none());
    }
}

#[tokio::test]
async fn a_key_is_sent_as_a_bearer_token() {
    for key in [KEY.to_string(), format!("  {KEY}\n")] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(RERANK))
            .and(header("Authorization", format!("Bearer {KEY}").as_str()))
            .respond_with(answer(json!([hit(0, 0.9)])))
            .expect(1)
            .mount(&server)
            .await;

        provider(HttpRerankConfig::new(url(&server), MODEL).with_api_key(key))
            .rerank(QUERY, &documents(&["first"]))
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn an_empty_batch_sends_nothing() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(answer(json!([hit(0, 0.9)])))
        .expect(0)
        .mount(&server)
        .await;

    let scores = provider(HttpRerankConfig::new(url(&server), MODEL))
        .rerank(QUERY, &[])
        .await
        .unwrap();

    assert!(scores.is_empty());
}

#[tokio::test]
async fn score_cuts_its_document_to_1200_characters() {
    let server = MockServer::start().await;
    serve(&server, answer(json!([hit(0, 0.75)]))).await;
    let document = "é".repeat(1500);

    let score = provider(HttpRerankConfig::new(url(&server), MODEL))
        .score(QUERY, &document)
        .await
        .unwrap();

    assert_eq!(score, 0.75);
    let body = sent_body(&server).await;
    assert_eq!(body["documents"], json!(["é".repeat(1200)]));
    assert_eq!(body["query"], json!(QUERY));
}

#[tokio::test]
async fn score_cuts_to_the_configured_number_of_characters() {
    let server = MockServer::start().await;
    serve(&server, answer(json!([hit(0, 0.75)]))).await;

    provider(HttpRerankConfig::new(url(&server), MODEL).with_maximum_document_characters(5))
        .score(QUERY, "fn should_skip_blob()")
        .await
        .unwrap();

    assert_eq!(sent_body(&server).await["documents"], json!(["fn sh"]));
}

#[tokio::test]
async fn rerank_sends_every_document_whole() {
    let server = MockServer::start().await;
    serve(&server, answer(json!([hit(0, 0.5)]))).await;
    let document = "x".repeat(1500);

    provider(HttpRerankConfig::new(url(&server), MODEL))
        .rerank(QUERY, std::slice::from_ref(&document))
        .await
        .unwrap();

    assert_eq!(sent_body(&server).await["documents"], json!([document]));
}

#[tokio::test]
async fn scores_come_back_in_input_order_whatever_order_the_hits_arrive_in() {
    let scores = scores_for(
        answer(json!([hit(2, 0.3), hit(0, 0.9), hit(1, 0.6)])),
        &["first", "second", "third"],
    )
    .await
    .unwrap();

    assert_eq!(scores, vec![0.9, 0.6, 0.3]);
}

#[tokio::test]
async fn a_score_field_is_read_when_relevance_score_is_absent() {
    let scores = scores_for(
        answer(json!([
            { "index": 0, "score": 0.4 },
            { "index": 1, "relevance_score": 0.8, "score": 0.2 },
            { "index": 2, "relevance_score": null, "score": 0.1 }
        ])),
        &["first", "second", "third"],
    )
    .await
    .unwrap();

    assert_eq!(scores, vec![0.4, 0.8, 0.1]);
}

#[tokio::test]
async fn a_hit_past_the_documents_is_ignored_and_missing_hits_score_zero() {
    let scores = scores_for(
        answer(json!([
            hit(5, 0.9),
            hit(1, 0.6),
            { "relevance_score": 0.7 },
            { "index": 2 }
        ])),
        &["first", "second", "third"],
    )
    .await
    .unwrap();

    assert_eq!(scores, vec![0.0, 0.6, 0.0]);
}

#[tokio::test]
async fn a_later_hit_for_the_same_document_wins() {
    let scores = scores_for(answer(json!([hit(0, 0.2), hit(0, 0.8)])), &["first"])
        .await
        .unwrap();

    assert_eq!(scores, vec![0.8]);
}

#[tokio::test]
async fn a_later_hit_without_a_score_keeps_the_score_already_read() {
    let scores = scores_for(
        answer(json!([hit(0, 0.9), { "index": 0 }])),
        &["first", "second"],
    )
    .await
    .unwrap();

    assert_eq!(scores, vec![0.9, 0.0]);
}

#[tokio::test]
async fn empty_or_missing_results_are_a_parse_error() {
    for body in [
        json!({ "results": [] }),
        json!({ "results": null }),
        json!({ "model": MODEL }),
    ] {
        let error = scores_for(ResponseTemplate::new(200).set_body_json(&body), &["first"])
            .await
            .unwrap_err();

        assert!(
            is_parse_error_saying(&error, "empty rerank response"),
            "{body}: {error:?}"
        );
        assert!(error.transient(), "{body}: an empty answer may not repeat");
    }
}

#[tokio::test]
async fn an_answer_that_does_not_decode_is_a_parse_error() {
    for response in [
        ResponseTemplate::new(200).set_body_raw("not json", "application/json"),
        ResponseTemplate::new(200).set_body_json(json!({ "results": "none" })),
        ResponseTemplate::new(200).set_body_json(json!({ "results": [{ "index": -1 }] })),
        ResponseTemplate::new(200)
            .set_body_json(json!({ "results": [{ "index": 0, "relevance_score": "high" }] })),
    ] {
        let error = scores_for(response, &["first"]).await.unwrap_err();

        assert!(matches!(error, ProviderError::Parse { .. }), "{error:?}");
    }
}

#[tokio::test]
async fn score_reads_the_single_score_back() {
    let server = MockServer::start().await;
    serve(&server, answer(json!([hit(0, 0.42)]))).await;

    let score = provider(HttpRerankConfig::new(url(&server), MODEL))
        .score(QUERY, "fn should_skip_blob()")
        .await
        .unwrap();

    assert_eq!(score, 0.42);
    assert_eq!(
        sent_body(&server).await["documents"],
        json!(["fn should_skip_blob()"])
    );
}

#[tokio::test]
async fn score_passes_an_empty_answer_on() {
    let server = MockServer::start().await;
    serve(&server, answer(json!([]))).await;

    let error = provider(HttpRerankConfig::new(url(&server), MODEL))
        .score(QUERY, "fn should_skip_blob()")
        .await
        .unwrap_err();

    assert!(
        is_parse_error_saying(&error, "empty rerank response"),
        "{error:?}"
    );
}

#[tokio::test]
async fn a_rejected_key_keeps_its_status() {
    for status in [401, 403] {
        let error = refusal(status, r#"{"error":{"message":"Invalid API key"}}"#).await;

        assert!(
            matches!(&error, ProviderError::Api { status: kept, message } if *kept == status && message.contains("Invalid API key")),
            "{error:?}"
        );
        assert!(!error.transient(), "{status} is not worth asking again");
    }
}

#[tokio::test]
async fn a_throttle_keeps_its_status_and_is_transient() {
    let error = refusal(429, r#"{"error":{"message":"Rate limit reached"}}"#).await;

    assert!(
        matches!(error, ProviderError::Api { status: 429, .. }),
        "{error:?}"
    );
    assert!(error.transient());
}

#[tokio::test]
async fn a_server_failure_keeps_its_status_and_body() {
    let error = refusal(503, "model is still loading").await;

    assert!(
        matches!(&error, ProviderError::Api { status: 503, message } if message == "model is still loading"),
        "{error:?}"
    );
    assert!(error.transient());
}

#[tokio::test]
async fn a_request_gives_up_at_its_timeout() {
    let server = MockServer::start().await;
    serve(
        &server,
        answer(json!([hit(0, 0.9)])).set_delay(Duration::from_millis(500)),
    )
    .await;

    let error = provider(
        HttpRerankConfig::new(url(&server), MODEL).with_timeout(Duration::from_millis(100)),
    )
    .rerank(QUERY, &documents(&["first"]))
    .await
    .unwrap_err();

    assert!(
        matches!(&error, ProviderError::Network { detail } if detail.starts_with("no answer within 100ms")),
        "{error:?}"
    );
    assert!(error.transient());
}

#[tokio::test]
async fn a_request_gives_up_after_two_and_a_half_seconds_by_default() {
    let server = MockServer::start().await;
    serve(
        &server,
        answer(json!([hit(0, 0.9)])).set_delay(Duration::from_millis(3500)),
    )
    .await;

    let error = provider(HttpRerankConfig::new(url(&server), MODEL))
        .rerank(QUERY, &documents(&["first"]))
        .await
        .unwrap_err();

    assert!(
        matches!(&error, ProviderError::Network { detail } if detail.starts_with("no answer within 2.5s")),
        "{error:?}"
    );
}

#[tokio::test]
async fn a_connection_dropped_before_an_answer_is_a_network_error() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((connection, _)) = listener.accept().await {
            drop(connection);
        }
    });

    let error = provider(HttpRerankConfig::new(
        format!("http://{address}{RERANK}"),
        MODEL,
    ))
    .rerank(QUERY, &documents(&["first"]))
    .await
    .unwrap_err();

    assert!(matches!(error, ProviderError::Network { .. }), "{error:?}");
    assert!(error.transient());
}

#[tokio::test]
async fn a_redirect_is_not_followed_and_surfaces_its_status() {
    let server = MockServer::start().await;
    let moved = "/v2/rerank";
    serve(
        &server,
        ResponseTemplate::new(302).insert_header("Location", format!("{}{moved}", server.uri())),
    )
    .await;
    Mock::given(method("POST"))
        .and(path(moved))
        .respond_with(answer(json!([hit(0, 0.9)])))
        .expect(0)
        .mount(&server)
        .await;

    let error = provider(HttpRerankConfig::new(url(&server), MODEL).with_api_key(KEY))
        .rerank(QUERY, &documents(&["first"]))
        .await
        .unwrap_err();

    assert!(
        matches!(error, ProviderError::Api { status: 302, .. }),
        "{error:?}"
    );
}

#[tokio::test]
async fn a_key_echoed_in_a_refusal_is_redacted() {
    let body = format!(
        r#"{{"error":{{"message":"Authentication Error, Invalid proxy server token passed. Received API Key = {SHAPELESS_KEY}"}}}}"#
    );

    let error = answered_with_shapeless_key(ResponseTemplate::new(401).set_body_string(body)).await;
    let rendered = error.to_string();

    assert!(
        matches!(error, ProviderError::Api { status: 401, .. }),
        "{error:?}"
    );
    assert!(!rendered.contains(SHAPELESS_KEY), "{rendered}");
    assert!(rendered.contains(REDACTED), "{rendered}");
    assert!(
        rendered.contains("Invalid proxy server token"),
        "{rendered}"
    );
}

#[tokio::test]
async fn a_padded_key_echoed_without_its_padding_is_redacted() {
    let server = MockServer::start().await;
    serve(
        &server,
        ResponseTemplate::new(403)
            .set_body_string(format!("caller {SHAPELESS_KEY} is not allowed")),
    )
    .await;

    let error = provider(
        HttpRerankConfig::new(url(&server), MODEL).with_api_key(format!("  {SHAPELESS_KEY}\n")),
    )
    .rerank(QUERY, &documents(&["first"]))
    .await
    .unwrap_err();
    let rendered = error.to_string();

    assert!(!rendered.contains(SHAPELESS_KEY), "{rendered}");
    assert!(rendered.contains(REDACTED), "{rendered}");
    let requests = server.received_requests().await.unwrap();
    assert_eq!(
        requests[0].headers.get("authorization").unwrap(),
        format!("Bearer {SHAPELESS_KEY}").as_str()
    );
}

#[tokio::test]
async fn a_key_echoed_in_an_answer_that_does_not_decode_is_redacted() {
    let body = json!({ "results": format!("rejected caller {SHAPELESS_KEY}") });

    let error = answered_with_shapeless_key(ResponseTemplate::new(200).set_body_json(body)).await;
    let rendered = error.to_string();

    assert!(matches!(error, ProviderError::Parse { .. }), "{error:?}");
    assert!(!rendered.contains(SHAPELESS_KEY), "{rendered}");
    assert!(rendered.contains(REDACTED), "{rendered}");
}

#[tokio::test]
async fn an_answer_that_scores_no_document_is_a_parse_error() {
    for results in [
        json!([{ "index": 0 }, { "index": 1 }]),
        json!([{ "relevance_score": 0.9 }, { "relevance_score": 0.1 }]),
        json!([hit(9, 0.9)]),
        json!([{ "index": 0, "rank_score": 0.9 }]),
    ] {
        let error = scores_for(answer(results.clone()), &["first", "second"])
            .await
            .unwrap_err();

        assert!(
            is_parse_error_saying(&error, "rerank response scored no document"),
            "{results}: {error:?}"
        );
    }
}

#[tokio::test]
async fn score_refuses_an_answer_that_scores_no_document() {
    let server = MockServer::start().await;
    serve(&server, answer(json!([hit(7, 0.9)]))).await;

    let error = provider(HttpRerankConfig::new(url(&server), MODEL))
        .score(QUERY, "fn should_skip_blob()")
        .await
        .unwrap_err();

    assert!(
        is_parse_error_saying(&error, "rerank response scored no document"),
        "{error:?}"
    );
}

#[tokio::test]
async fn negative_scores_pass_through_on_the_model_scale() {
    let scores = scores_for(answer(json!([hit(0, -3.2)])), &["first", "second"])
        .await
        .unwrap();

    assert_eq!(scores, vec![-3.2, 0.0]);
}

#[tokio::test]
async fn a_hit_without_an_index_is_not_placed_by_its_position() {
    let scores = scores_for(
        answer(json!([{ "relevance_score": 0.7 }, hit(1, 0.6)])),
        &["first", "second"],
    )
    .await
    .unwrap();

    assert_eq!(scores, vec![0.0, 0.6]);
}

#[tokio::test]
async fn the_query_string_of_the_url_is_kept() {
    let server = MockServer::start().await;
    serve(&server, answer(json!([hit(0, 0.9)]))).await;

    provider(HttpRerankConfig::new(
        format!("{}?api-version=1", url(&server)),
        MODEL,
    ))
    .rerank(QUERY, &documents(&["first"]))
    .await
    .unwrap();

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests[0].url.query(), Some("api-version=1"));
}

#[tokio::test]
async fn a_success_with_no_body_is_a_parse_error() {
    for response in [ResponseTemplate::new(200), ResponseTemplate::new(204)] {
        let error = scores_for(response, &["first"]).await.unwrap_err();

        assert!(matches!(error, ProviderError::Parse { .. }), "{error:?}");
    }
}

#[test]
fn a_url_that_is_not_http_is_refused() {
    for url in ["ftp://example.com/v1/rerank", "not a url", ""] {
        let error = HttpRerankProvider::new(HttpRerankConfig::new(url, MODEL)).unwrap_err();

        assert!(matches!(error, ProviderError::Config { .. }), "{error:?}");
    }
}

#[test]
fn a_config_never_prints_its_key() {
    let config = HttpRerankConfig::new("http://localhost:8080/v1/rerank", MODEL).with_api_key(KEY);
    let provider = HttpRerankProvider::new(config.clone()).unwrap();

    for printed in [format!("{config:?}"), format!("{provider:?}")] {
        assert!(!printed.contains(KEY), "{printed}");
        assert!(printed.contains(REDACTED), "{printed}");
    }
}

#[test]
fn neither_print_shows_more_of_the_url_than_its_scheme_host_and_port() {
    for url in [
        "http://plain-user:plain-secret@localhost:8080/v1/rerank?api_key=plain-secret#plain-secret",
        "http://plain-user@localhost:8080/v1/rerank",
        "http://localhost:8080/bot:plain-secret@host/v1/rerank",
        "http://localhost:8080/key/plain-secret/rerank",
        "http://localhost:8080/v1/rerank;token=plain-secret",
    ] {
        let config = HttpRerankConfig::new(url, MODEL);
        let provider = HttpRerankProvider::new(config.clone()).unwrap();

        for printed in [format!("{config:?}"), format!("{provider:?}")] {
            assert!(!printed.contains("plain-secret"), "{printed}");
            assert!(!printed.contains("plain-user"), "{printed}");
            assert!(printed.contains("http://localhost:8080"), "{printed}");
            assert!(printed.contains(MODEL), "{printed}");
        }
    }
}

#[test]
fn a_refused_url_is_not_repeated_in_the_error() {
    for url in [
        "not a url plain-secret",
        "ftp://plain-secret/rerank",
        "ftp://host/plain-secret",
        "ftp://plain-secret@host/rerank",
    ] {
        let error = HttpRerankProvider::new(HttpRerankConfig::new(url, MODEL)).unwrap_err();
        let rendered = error.to_string();

        assert!(matches!(error, ProviderError::Config { .. }), "{error:?}");
        assert!(!rendered.contains("plain-secret"), "{rendered}");
    }
}

#[test]
fn a_config_whose_url_does_not_parse_does_not_print_it() {
    let printed = format!(
        "{:?}",
        HttpRerankConfig::new("not a url plain-secret", MODEL)
    );

    assert!(!printed.contains("plain-secret"), "{printed}");
}

#[tokio::test]
async fn a_refusal_whose_body_stalls_past_the_deadline_keeps_its_status() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = [0u8; 4096];
        let _ = stream.read(&mut request).await.unwrap();
        stream
            .write_all(b"HTTP/1.1 401 Unauthorized\r\ncontent-length: 100\r\n\r\npartial")
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_secs(5)).await;
        drop(stream);
    });

    let error = provider(
        HttpRerankConfig::new(format!("http://{address}{RERANK}"), MODEL)
            .with_timeout(Duration::from_millis(300)),
    )
    .rerank(QUERY, &documents(&["first"]))
    .await
    .unwrap_err();
    server.abort();

    assert!(
        matches!(&error, ProviderError::Api { status: 401, message } if message.contains("300ms")),
        "{error:?}"
    );
    assert!(!error.transient(), "{error:?}");
}

#[test]
fn a_config_starts_from_the_defaults() {
    let config = HttpRerankConfig::new("http://localhost:8080/v1/rerank", MODEL);

    assert_eq!(config.url, "http://localhost:8080/v1/rerank");
    assert_eq!(config.model, MODEL);
    assert!(config.api_key.is_none());
    assert_eq!(config.timeout, Duration::from_millis(2500));
    assert_eq!(config.maximum_document_characters, 1200);
}

#[test]
fn builders_replace_the_fields_they_name() {
    let config = HttpRerankConfig::new("http://localhost:8080/v1/rerank", MODEL)
        .with_api_key(KEY)
        .with_timeout(Duration::from_secs(5))
        .with_maximum_document_characters(512);

    assert_eq!(config.api_key.as_ref().map(SecretValue::expose), Some(KEY));
    assert_eq!(config.timeout, Duration::from_secs(5));
    assert_eq!(config.maximum_document_characters, 512);
}

#[test]
fn the_provider_is_a_rerank_provider() {
    let boxed: Box<dyn RerankProvider> = Box::new(provider(HttpRerankConfig::new(
        "http://localhost:8080/v1/rerank",
        MODEL,
    )));

    assert_eq!(boxed.name(), "http-rerank");
    assert_eq!(boxed.model(), MODEL);
}
