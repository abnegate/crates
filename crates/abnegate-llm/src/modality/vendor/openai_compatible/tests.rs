use std::time::Duration;

use abnegate_secret::REDACTED;
use abnegate_secret::SecretValue;
use serde_json::Value;
use serde_json::json;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;
use wiremock::matchers::header;
use wiremock::matchers::method;
use wiremock::matchers::path;

use super::*;

const MODEL: &str = "qwen3-embedding:0.6b";
const EMBEDDINGS: &str = "/v1/embeddings";
const KEY: &str = "sk-litellm-master-0123456789";
const CONTEXT_LENGTH_PHRASES: [&str; 4] = [
    "context length",
    "context window",
    "input length",
    "too many tokens",
];
const OPENAI_CONTEXT_LENGTH: &str = r#"{"error":{"message":"This model's maximum context length is 8192 tokens, however you requested 9013 tokens (9013 in your prompt; 0 for the completion). Please reduce your prompt; or completion length.","type":"invalid_request_error","param":null,"code":null}}"#;
const LITELLM_CONTEXT_WINDOW: &str = r#"{"error":{"message":"litellm.ContextWindowExceededError: litellm.BadRequestError: OpenAIException - This model's maximum context length is 8192 tokens, however you requested 9013 tokens (9013 in your prompt; 0 for the completion). Please reduce your prompt; or completion length.","type":null,"param":null,"code":"400"}}"#;
const OLLAMA_CONTEXT_LENGTH: &str = r#"{"error":{"message":"the input length exceeds the context length","type":"api_error","param":null,"code":null}}"#;

fn texts(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_string()).collect()
}

fn base(server: &MockServer) -> String {
    format!("{}/v1", server.uri())
}

fn provider(config: OpenAiCompatibleEmbeddingConfig) -> OpenAiCompatibleEmbeddingProvider {
    OpenAiCompatibleEmbeddingProvider::new(config).unwrap()
}

fn answer(items: Value) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "object": "list",
        "data": items,
        "model": MODEL,
        "usage": { "prompt_tokens": 2, "total_tokens": 2 }
    }))
}

fn item(index: usize, embedding: &[f32]) -> Value {
    json!({ "object": "embedding", "index": index, "embedding": embedding })
}

async fn serve(server: &MockServer, response: ResponseTemplate) {
    Mock::given(method("POST"))
        .and(path(EMBEDDINGS))
        .respond_with(response)
        .mount(server)
        .await;
}

async fn sent_body(server: &MockServer) -> Value {
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1, "exactly one request is sent");
    serde_json::from_slice(&requests[0].body).unwrap()
}

async fn refusal(status: u16, body: &str) -> ProviderError {
    refusal_with_key(status, body, KEY).await
}

async fn refusal_with_key(status: u16, body: &str, key: &str) -> ProviderError {
    let server = MockServer::start().await;
    serve(
        &server,
        ResponseTemplate::new(status).set_body_raw(body.to_string(), "application/json"),
    )
    .await;
    provider(OpenAiCompatibleEmbeddingConfig::new(base(&server), MODEL).with_api_key(key))
        .embed(&texts(&["hello"]))
        .await
        .unwrap_err()
}

fn reads_as_context_length(error: &ProviderError) -> bool {
    let message = error.to_string().to_ascii_lowercase();
    CONTEXT_LENGTH_PHRASES
        .iter()
        .any(|phrase| message.contains(phrase))
}

#[tokio::test]
async fn the_body_carries_the_model_and_every_input() {
    let server = MockServer::start().await;
    serve(&server, answer(json!([item(0, &[0.1]), item(1, &[0.2])]))).await;

    provider(OpenAiCompatibleEmbeddingConfig::new(base(&server), MODEL))
        .embed(&texts(&["first", "second"]))
        .await
        .unwrap();

    assert_eq!(
        sent_body(&server).await,
        json!({ "model": MODEL, "input": ["first", "second"] })
    );
}

#[tokio::test]
async fn dimensions_are_sent_only_when_configured() {
    let unset = MockServer::start().await;
    serve(&unset, answer(json!([item(0, &[0.1])]))).await;
    provider(OpenAiCompatibleEmbeddingConfig::new(base(&unset), MODEL))
        .embed(&texts(&["hello"]))
        .await
        .unwrap();
    assert!(sent_body(&unset).await.get("dimensions").is_none());

    let set = MockServer::start().await;
    serve(&set, answer(json!([item(0, &[0.1])]))).await;
    provider(OpenAiCompatibleEmbeddingConfig::new(base(&set), MODEL).with_dimensions(256))
        .embed(&texts(&["hello"]))
        .await
        .unwrap();
    assert_eq!(sent_body(&set).await["dimensions"], json!(256));
}

#[tokio::test]
async fn a_key_is_sent_as_a_bearer_token() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(EMBEDDINGS))
        .and(header("Authorization", format!("Bearer {KEY}").as_str()))
        .respond_with(answer(json!([item(0, &[0.1])])))
        .expect(1)
        .mount(&server)
        .await;

    provider(OpenAiCompatibleEmbeddingConfig::new(base(&server), MODEL).with_api_key(KEY))
        .embed(&texts(&["hello"]))
        .await
        .unwrap();
}

#[tokio::test]
async fn no_authorization_is_sent_without_a_key() {
    for config in [
        OpenAiCompatibleEmbeddingConfig::new("", MODEL),
        OpenAiCompatibleEmbeddingConfig::new("", MODEL).with_api_key(""),
    ] {
        let server = MockServer::start().await;
        serve(&server, answer(json!([item(0, &[0.1])]))).await;
        let config = OpenAiCompatibleEmbeddingConfig {
            base_url: base(&server),
            ..config
        };

        provider(config).embed(&texts(&["hello"])).await.unwrap();

        let requests = server.received_requests().await.unwrap();
        assert!(requests[0].headers.get("authorization").is_none());
    }
}

#[tokio::test]
async fn embeddings_are_returned_in_input_order_whatever_order_they_arrive_in() {
    let server = MockServer::start().await;
    serve(
        &server,
        answer(json!([item(1, &[0.2, 0.2]), item(0, &[0.1, 0.1])])),
    )
    .await;

    let embeddings = provider(OpenAiCompatibleEmbeddingConfig::new(base(&server), MODEL))
        .embed(&texts(&["first", "second"]))
        .await
        .unwrap();

    assert_eq!(embeddings, vec![vec![0.1, 0.1], vec![0.2, 0.2]]);
}

#[tokio::test]
async fn an_answer_with_the_wrong_number_of_embeddings_is_refused() {
    let server = MockServer::start().await;
    serve(&server, answer(json!([item(0, &[0.1])]))).await;

    let error = provider(OpenAiCompatibleEmbeddingConfig::new(base(&server), MODEL))
        .embed(&texts(&["first", "second"]))
        .await
        .unwrap_err();

    assert!(matches!(error, ProviderError::Parse { .. }), "{error:?}");
}

#[tokio::test]
async fn a_duplicate_index_is_refused() {
    let server = MockServer::start().await;
    serve(&server, answer(json!([item(0, &[0.1]), item(0, &[0.2])]))).await;

    let error = provider(OpenAiCompatibleEmbeddingConfig::new(base(&server), MODEL))
        .embed(&texts(&["first", "second"]))
        .await
        .unwrap_err();

    assert!(matches!(error, ProviderError::Parse { .. }), "{error:?}");
}

#[tokio::test]
async fn an_index_past_the_inputs_is_refused() {
    let server = MockServer::start().await;
    serve(&server, answer(json!([item(0, &[0.1]), item(2, &[0.2])]))).await;

    let error = provider(OpenAiCompatibleEmbeddingConfig::new(base(&server), MODEL))
        .embed(&texts(&["first", "second"]))
        .await
        .unwrap_err();

    assert!(matches!(error, ProviderError::Parse { .. }), "{error:?}");
}

#[tokio::test]
async fn an_item_without_an_index_is_refused() {
    let server = MockServer::start().await;
    serve(
        &server,
        answer(json!([{ "object": "embedding", "embedding": [0.1] }])),
    )
    .await;

    let error = provider(OpenAiCompatibleEmbeddingConfig::new(base(&server), MODEL))
        .embed(&texts(&["hello"]))
        .await
        .unwrap_err();

    assert!(matches!(error, ProviderError::Parse { .. }), "{error:?}");
}

#[tokio::test]
async fn an_empty_batch_sends_nothing() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(answer(json!([])))
        .expect(0)
        .mount(&server)
        .await;

    let embeddings = provider(OpenAiCompatibleEmbeddingConfig::new(base(&server), MODEL))
        .embed(&[])
        .await
        .unwrap();

    assert!(embeddings.is_empty());
}

#[tokio::test]
async fn an_unconfigured_width_is_learned_from_the_first_answer() {
    let server = MockServer::start().await;
    serve(&server, answer(json!([item(0, &[0.1, 0.2, 0.3])]))).await;
    let learning = provider(OpenAiCompatibleEmbeddingConfig::new(base(&server), MODEL));
    let configured =
        provider(OpenAiCompatibleEmbeddingConfig::new(base(&server), MODEL).with_dimensions(1024));

    assert_eq!(learning.dimensions(), 0);
    learning.embed(&texts(&["hello"])).await.unwrap();
    configured.embed(&texts(&["hello"])).await.unwrap();

    assert_eq!(learning.dimensions(), 3);
    assert_eq!(configured.dimensions(), 1024);
}

#[tokio::test]
async fn the_width_is_learned_from_the_first_answer_only() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(EMBEDDINGS))
        .respond_with(answer(json!([item(0, &[0.1, 0.2, 0.3])])))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    serve(&server, answer(json!([item(0, &[0.1, 0.2])]))).await;
    let learning = provider(OpenAiCompatibleEmbeddingConfig::new(base(&server), MODEL));

    let first = learning.embed(&texts(&["first"])).await.unwrap();
    let second = learning.embed(&texts(&["second"])).await.unwrap();

    assert_eq!(first[0].len(), 3);
    assert_eq!(second[0].len(), 2);
    assert_eq!(learning.dimensions(), 3, "a later answer moved the width");
}

#[tokio::test]
async fn an_empty_embedding_is_refused() {
    let server = MockServer::start().await;
    serve(&server, answer(json!([item(0, &[0.1]), item(1, &[])]))).await;
    let provider = provider(OpenAiCompatibleEmbeddingConfig::new(base(&server), MODEL));

    let error = provider
        .embed(&texts(&["first", "second"]))
        .await
        .unwrap_err();

    assert!(matches!(error, ProviderError::Parse { .. }), "{error:?}");
    assert_eq!(provider.dimensions(), 0, "a refused answer taught a width");
}

#[tokio::test]
async fn a_single_text_is_embedded_alone() {
    let server = MockServer::start().await;
    serve(&server, answer(json!([item(0, &[0.5, 0.25])]))).await;

    let embedding = provider(OpenAiCompatibleEmbeddingConfig::new(base(&server), MODEL))
        .embed_single("hello")
        .await
        .unwrap();

    assert_eq!(embedding, vec![0.5, 0.25]);
    assert_eq!(sent_body(&server).await["input"], json!(["hello"]));
}

#[tokio::test]
async fn a_rejected_key_keeps_its_status() {
    for status in [401, 403] {
        let error = refusal(status, r#"{"error":{"message":"Invalid API key"}}"#).await;

        assert!(
            matches!(error, ProviderError::Api { status: kept, .. } if kept == status),
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
async fn a_context_window_rejection_still_reads_as_one() {
    for (status, body) in [
        (400, OPENAI_CONTEXT_LENGTH),
        (400, LITELLM_CONTEXT_WINDOW),
        (500, OLLAMA_CONTEXT_LENGTH),
    ] {
        let error = refusal(status, body).await;

        assert!(
            matches!(error, ProviderError::Api { status: kept, .. } if kept == status),
            "{error:?}"
        );
        assert!(
            reads_as_context_length(&error),
            "{error} no longer reads as a context-window rejection"
        );
    }
}

#[tokio::test]
async fn a_short_key_never_hides_a_context_length_rejection() {
    for key in ["x", "token", "len"] {
        for (status, body, phrase) in [
            (400, OPENAI_CONTEXT_LENGTH, "context length"),
            (400, LITELLM_CONTEXT_WINDOW, "context length"),
            (500, OLLAMA_CONTEXT_LENGTH, "input length"),
        ] {
            let error = refusal_with_key(status, body, key).await;
            let rendered = error.to_string().to_ascii_lowercase();

            assert!(
                rendered.contains(phrase),
                "the key {key:?} hid {phrase:?}: {rendered}"
            );
            assert!(reads_as_context_length(&error), "{key:?}: {rendered}");
        }
    }
}

#[tokio::test]
async fn a_key_echoed_in_a_refusal_is_redacted() {
    let masked = format!("{}****{}", &KEY[..6], &KEY[KEY.len() - 4..]);
    for echoed in [KEY.to_string(), masked] {
        let body = format!(
            r#"{{"error":{{"message":"Authentication Error, Invalid proxy server token passed. Received API Key = {echoed}"}}}}"#
        );

        let error = refusal(401, &body).await;
        let rendered = error.to_string();

        assert!(!rendered.contains(&echoed), "{rendered}");
        assert!(rendered.contains(REDACTED), "{rendered}");
        assert!(
            rendered.contains("Invalid proxy server token"),
            "{rendered}"
        );
    }
}

#[tokio::test]
async fn a_short_key_echoed_in_a_refusal_is_redacted() {
    let key = concat!("sk-", "1234");
    let body = format!(
        r#"{{"error":{{"message":"Authentication Error, Invalid proxy server token passed. Received API Key = {key}, Key Hash (Token) =9f86d081884c7d65","type":"auth_error","param":"None","code":"401"}}}}"#
    );

    let error = refusal_with_key(401, &body, key).await;
    let rendered = error.to_string();

    assert!(
        matches!(error, ProviderError::Api { status: 401, .. }),
        "{error:?}"
    );
    assert!(!rendered.contains(key), "{rendered}");
    assert!(
        rendered.contains(&format!("Received API Key = {REDACTED}, Key Hash")),
        "{rendered}"
    );
}

#[test]
fn a_base_url_that_is_not_http_is_refused() {
    for base_url in ["ftp://example.com/v1", "not a url"] {
        let error = OpenAiCompatibleEmbeddingProvider::new(OpenAiCompatibleEmbeddingConfig::new(
            base_url, MODEL,
        ))
        .unwrap_err();

        assert!(matches!(error, ProviderError::Config { .. }), "{error:?}");
    }
}

#[tokio::test]
async fn a_trailing_slash_on_the_base_url_is_ignored() {
    let server = MockServer::start().await;
    serve(&server, answer(json!([item(0, &[0.1])]))).await;

    provider(OpenAiCompatibleEmbeddingConfig::new(
        format!("{}/v1/", server.uri()),
        MODEL,
    ))
    .embed(&texts(&["hello"]))
    .await
    .unwrap();

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests[0].url.path(), EMBEDDINGS);
}

#[tokio::test]
async fn the_base_url_already_carries_v1() {
    let server = MockServer::start().await;
    serve(&server, answer(json!([item(0, &[0.1])]))).await;

    provider(OpenAiCompatibleEmbeddingConfig::new(base(&server), MODEL))
        .embed(&texts(&["hello"]))
        .await
        .unwrap();

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].url.path(), EMBEDDINGS);
}

#[tokio::test]
async fn a_request_gives_up_at_its_timeout() {
    let server = MockServer::start().await;
    serve(
        &server,
        answer(json!([item(0, &[0.1])])).set_delay(Duration::from_millis(500)),
    )
    .await;

    let error = provider(
        OpenAiCompatibleEmbeddingConfig::new(base(&server), MODEL)
            .with_timeout(Duration::from_millis(100)),
    )
    .embed(&texts(&["hello"]))
    .await
    .unwrap_err();

    assert!(matches!(error, ProviderError::Network { .. }), "{error:?}");
}

#[test]
fn a_config_never_prints_its_key() {
    let config =
        OpenAiCompatibleEmbeddingConfig::new("http://localhost:4000/v1", MODEL).with_api_key(KEY);
    let provider = OpenAiCompatibleEmbeddingProvider::new(config.clone()).unwrap();

    for printed in [format!("{config:?}"), format!("{provider:?}")] {
        assert!(!printed.contains(KEY), "{printed}");
        assert!(printed.contains(REDACTED), "{printed}");
    }
}

#[test]
fn a_config_starts_from_the_defaults() {
    let config = OpenAiCompatibleEmbeddingConfig::new("http://localhost:4000/v1", MODEL);

    assert_eq!(config.base_url, "http://localhost:4000/v1");
    assert_eq!(config.model, MODEL);
    assert!(config.api_key.is_none());
    assert_eq!(config.dimensions, None);
    assert_eq!(config.maximum_input_tokens, 512);
    assert_eq!(config.timeout, Duration::from_secs(30));
}

#[test]
fn builders_replace_the_fields_they_name() {
    let config = OpenAiCompatibleEmbeddingConfig::new("http://localhost:4000/v1", MODEL)
        .with_api_key(KEY)
        .with_dimensions(1024)
        .with_maximum_input_tokens(8192)
        .with_timeout(Duration::from_secs(5));

    assert_eq!(config.api_key.as_ref().map(SecretValue::expose), Some(KEY));
    assert_eq!(config.dimensions, Some(1024));
    assert_eq!(config.maximum_input_tokens, 8192);
    assert_eq!(config.timeout, Duration::from_secs(5));
}

#[test]
fn a_provider_reports_its_name_model_and_input_limit() {
    let provider = provider(
        OpenAiCompatibleEmbeddingConfig::new("http://localhost:4000/v1", MODEL)
            .with_maximum_input_tokens(8192),
    );

    assert_eq!(provider.name(), "openai-compatible");
    assert_eq!(provider.model(), MODEL);
    assert_eq!(provider.maximum_input_tokens(), 8192);
    assert_eq!(provider.maximum_input_characters(), 3000);
}

#[test]
fn the_provider_is_an_embedding_provider() {
    let boxed: Box<dyn EmbeddingProvider> = Box::new(provider(
        OpenAiCompatibleEmbeddingConfig::new("http://localhost:4000/v1", MODEL),
    ));

    assert_eq!(boxed.name(), "openai-compatible");
}
