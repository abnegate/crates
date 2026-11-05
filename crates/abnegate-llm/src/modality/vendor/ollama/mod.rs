//! Ollama's native embeddings API.

mod config;
mod request;
mod response;

use std::time::Duration;

use abnegate_secret::SecretValue;
use async_trait::async_trait;
use futures::StreamExt;
use futures::TryStreamExt;
use futures::stream;
use reqwest::Url;

pub use crate::modality::vendor::ollama::config::OllamaEmbeddingConfig;

use crate::client::reporter::conceal_key;
use crate::modality::EmbeddingProvider;
use crate::modality::embedding_provider::DEFAULT_MAXIMUM_INPUT_TOKENS;
use crate::modality::vendor::endpoint::endpoint;
use crate::modality::vendor::ollama::request::Request;
use crate::modality::vendor::ollama::response::Response;
use crate::modality::vendor::transport::Transport;
use crate::provider::ProviderError;

const NAME: &str = "ollama";
const PATH: &str = "/api/embeddings";
const SHORT_PROMPT_CHARACTERS: usize = 2000;
const DEFAULT_DIMENSIONS: u32 = 768;
const QWEN3_EMBEDDING: &str = "qwen3-embedding";
const SECONDS_PER_MINUTE: u64 = 60;
const NANOSECONDS_PER_MILLISECOND: u32 = 1_000_000;

/// Embeddings from an Ollama server's native API.
///
/// Each prompt is one `POST` to `/api/embeddings`, which answers with the
/// model's raw vector, and never to `/api/embed`, which normalises it, so
/// vectors stored from either side compare equal. A batch keeps up to
/// [`OllamaEmbeddingConfig::concurrency`] prompts in flight and comes back in
/// input order. A prompt under 2,000 characters gets the short deadline, and
/// any longer one the long deadline.
///
/// A refusal is a [`ProviderError::Api`] holding Ollama's status and its
/// body, with the configured key and any other credential scrubbed out, so a caller can tell a rejected key, a throttled request
/// or an input past the model's context length apart. The vector is passed
/// on at whatever width Ollama returns; compare its length with
/// [`EmbeddingProvider::dimensions`] where the width matters. An empty
/// vector, which Ollama sends for a model that cannot embed, is refused as
/// [`ProviderError::Parse`].
#[derive(Debug, Clone)]
pub struct OllamaEmbeddingProvider {
    url: Url,
    model: String,
    api_key: Option<SecretValue>,
    dimensions: u32,
    maximum_input_tokens: u32,
    keep_alive: Option<String>,
    concurrency: usize,
    short: Transport,
    long: Transport,
}

impl OllamaEmbeddingProvider {
    /// A provider for the server and model `config` names, failing with
    /// [`ProviderError::Config`] when its base URL does not parse or is not
    /// `http` or `https`.
    pub fn new(config: OllamaEmbeddingConfig) -> Result<Self, ProviderError> {
        let url = endpoint(&config.base_url, PATH)?;
        Ok(Self {
            url,
            dimensions: config
                .dimensions
                .unwrap_or_else(|| model_dimensions(&config.model)),
            maximum_input_tokens: model_maximum_input_tokens(&config.model),
            api_key: config.api_key.filter(|key| !key.is_empty()),
            keep_alive: config.keep_alive.map(render_keep_alive),
            concurrency: config.concurrency.max(1),
            short: Transport::with_timeout(config.short_prompt_timeout),
            long: Transport::with_timeout(config.timeout),
            model: config.model,
        })
    }

    async fn embed_prompt(&self, prompt: &str) -> Result<Vec<f32>, ProviderError> {
        let transport = if is_short(prompt) {
            &self.short
        } else {
            &self.long
        };
        let mut request = transport.post(self.url.clone()).json(&Request {
            model: &self.model,
            prompt,
            keep_alive: self.keep_alive.as_deref(),
        });
        if let Some(key) = &self.api_key {
            request = request.bearer_auth(key.expose());
        }
        let body = transport
            .send(request)
            .await
            .map_err(|error| self.concealed(error))?;
        let response: Response = serde_json::from_value(body).map_err(ProviderError::parse)?;
        if response.embedding.is_empty() {
            return Err(ProviderError::parse(format!(
                "Ollama sent an empty embedding, so {} may not be an embedding model",
                self.model
            )));
        }
        Ok(response.embedding)
    }

    fn concealed(&self, error: ProviderError) -> ProviderError {
        match (&self.api_key, error) {
            (Some(key), ProviderError::Api { status, message }) => {
                ProviderError::api(status, conceal_key(&message, key.expose()))
            }
            (_, error) => error,
        }
    }
}

#[async_trait]
impl EmbeddingProvider for OllamaEmbeddingProvider {
    fn name(&self) -> &str {
        NAME
    }

    fn dimensions(&self) -> u32 {
        self.dimensions
    }

    fn model(&self) -> &str {
        &self.model
    }

    fn maximum_input_tokens(&self) -> u32 {
        self.maximum_input_tokens
    }

    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, ProviderError> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        stream::iter(0..texts.len())
            .map(|index| self.embed_prompt(&texts[index]))
            .buffered(self.concurrency)
            .try_collect()
            .await
    }

    async fn embed_single(&self, text: &str) -> Result<Vec<f32>, ProviderError> {
        self.embed_prompt(text).await
    }
}

fn is_short(prompt: &str) -> bool {
    prompt.chars().take(SHORT_PROMPT_CHARACTERS).count() < SHORT_PROMPT_CHARACTERS
}

fn render_keep_alive(keep_alive: Duration) -> String {
    let seconds = keep_alive.as_secs();
    let nanoseconds = keep_alive.subsec_nanos();
    if nanoseconds == 0 && seconds.is_multiple_of(SECONDS_PER_MINUTE) {
        format!("{}m", seconds / SECONDS_PER_MINUTE)
    } else if nanoseconds == 0 {
        format!("{seconds}s")
    } else if nanoseconds.is_multiple_of(NANOSECONDS_PER_MILLISECOND) {
        format!("{}ms", keep_alive.as_millis())
    } else {
        format!("{}ns", keep_alive.as_nanos())
    }
}

fn model_base(model: &str) -> String {
    model
        .split(':')
        .next()
        .unwrap_or(model)
        .to_ascii_lowercase()
}

fn model_dimensions(model: &str) -> u32 {
    let base = model_base(model);
    if base.contains(QWEN3_EMBEDDING) {
        return 1024;
    }
    match base.as_str() {
        "nomic-embed-text" | "nomic-embed-text-v1.5" | "nomic-embed-text-v1" => 768,
        "mxbai-embed-large" | "snowflake-arctic-embed" => 1024,
        "bge-small-en" | "bge-small-en-v1.5" | "all-minilm" | "all-minilm-l6-v2" => 384,
        _ => DEFAULT_DIMENSIONS,
    }
}

fn model_maximum_input_tokens(model: &str) -> u32 {
    let base = model_base(model);
    if base.starts_with(QWEN3_EMBEDDING) {
        return 8192;
    }
    match base.as_str() {
        "nomic-embed-text" | "nomic-embed-text-v1.5" | "nomic-embed-text-v1" => 8192,
        "all-minilm" | "all-minilm-l6-v2" => 256,
        _ => DEFAULT_MAXIMUM_INPUT_TOKENS,
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use abnegate_secret::REDACTED;
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::body_partial_json;
    use wiremock::matchers::header;
    use wiremock::matchers::method;
    use wiremock::matchers::path;

    use super::*;

    const MODEL: &str = "nomic-embed-text";
    const KEY: &str = concat!("sk-", "ollama-proxy-0123456789abcdef");
    const HANG: Duration = Duration::from_secs(5);

    fn config(server: &MockServer) -> OllamaEmbeddingConfig {
        OllamaEmbeddingConfig::new(server.uri(), MODEL)
    }

    fn provider(config: OllamaEmbeddingConfig) -> OllamaEmbeddingProvider {
        OllamaEmbeddingProvider::new(config).unwrap()
    }

    fn embedding(values: &[f32]) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(serde_json::json!({ "embedding": values }))
    }

    async fn answering(values: &[f32]) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(PATH))
            .respond_with(embedding(values))
            .mount(&server)
            .await;
        server
    }

    fn prompts(count: usize) -> Vec<String> {
        (0..count).map(|index| format!("prompt {index}")).collect()
    }

    #[tokio::test]
    async fn the_request_body_matches_what_zone_sends_byte_for_byte() {
        let server = answering(&[0.5]).await;

        provider(config(&server))
            .embed_single("hello")
            .await
            .unwrap();

        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].url.path(), "/api/embeddings");
        assert_eq!(
            String::from_utf8(requests[0].body.clone()).unwrap(),
            r#"{"model":"nomic-embed-text","prompt":"hello","keep_alive":"30m"}"#
        );
    }

    #[tokio::test]
    async fn no_keep_alive_leaves_the_field_out() {
        let server = answering(&[0.5]).await;
        let mut config = config(&server);
        config.keep_alive = None;

        provider(config).embed_single("hello").await.unwrap();

        let requests = server.received_requests().await.unwrap();
        assert_eq!(
            String::from_utf8(requests[0].body.clone()).unwrap(),
            r#"{"model":"nomic-embed-text","prompt":"hello"}"#
        );
    }

    #[tokio::test]
    async fn a_key_is_sent_as_a_bearer_token() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(PATH))
            .and(header("Authorization", format!("Bearer {KEY}").as_str()))
            .respond_with(embedding(&[0.5]))
            .expect(1)
            .mount(&server)
            .await;

        let embedded = provider(config(&server).with_api_key(KEY))
            .embed_single("hello")
            .await
            .unwrap();

        assert_eq!(embedded, vec![0.5]);
    }

    #[tokio::test]
    async fn no_authorization_is_sent_without_a_key() {
        let server = answering(&[0.5]).await;

        provider(config(&server))
            .embed_single("hello")
            .await
            .unwrap();

        let requests = server.received_requests().await.unwrap();
        assert!(!requests[0].headers.contains_key("authorization"));
    }

    #[tokio::test]
    async fn an_empty_key_sends_no_authorization() {
        let server = answering(&[0.5]).await;

        provider(config(&server).with_api_key(""))
            .embed_single("hello")
            .await
            .unwrap();

        let requests = server.received_requests().await.unwrap();
        assert!(!requests[0].headers.contains_key("authorization"));
    }

    #[tokio::test]
    async fn embeddings_come_back_in_input_order() {
        let server = MockServer::start().await;
        for (prompt, value, delay) in [("one", 1.0, 300), ("two", 2.0, 150), ("three", 3.0, 0)] {
            Mock::given(method("POST"))
                .and(path(PATH))
                .and(body_partial_json(serde_json::json!({ "prompt": prompt })))
                .respond_with(embedding(&[value]).set_delay(Duration::from_millis(delay)))
                .mount(&server)
                .await;
        }
        let texts = ["one", "two", "three"].map(String::from);

        let embeddings = provider(config(&server)).embed(&texts).await.unwrap();

        assert_eq!(embeddings, vec![vec![1.0], vec![2.0], vec![3.0]]);
    }

    #[tokio::test]
    async fn at_most_the_configured_number_of_prompts_are_in_flight() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(PATH))
            .respond_with(embedding(&[0.5]).set_delay(Duration::from_millis(300)))
            .mount(&server)
            .await;
        let provider = provider(config(&server).with_concurrency(10));
        let started = Instant::now();

        let embeddings = provider.embed(&prompts(20)).await.unwrap();

        let elapsed = started.elapsed();
        assert_eq!(embeddings.len(), 20);
        assert!(
            elapsed >= Duration::from_millis(600),
            "more than 10 prompts were in flight: {elapsed:?}"
        );
        assert!(
            elapsed < Duration::from_secs(3),
            "the prompts did not run in parallel: {elapsed:?}"
        );
    }

    #[tokio::test]
    async fn a_concurrency_of_zero_still_embeds() {
        let server = answering(&[0.5]).await;
        let provider = provider(config(&server).with_concurrency(0));

        let embeddings = tokio::time::timeout(HANG, provider.embed(&prompts(3)))
            .await
            .expect("a concurrency of zero never sent a prompt")
            .unwrap();

        assert_eq!(embeddings.len(), 3);
    }

    #[tokio::test]
    async fn an_empty_batch_sends_nothing() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(embedding(&[0.5]))
            .expect(0)
            .mount(&server)
            .await;

        let embeddings = provider(config(&server)).embed(&[]).await.unwrap();

        assert!(embeddings.is_empty());
    }

    async fn slow_server() -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(PATH))
            .respond_with(embedding(&[0.5]).set_delay(Duration::from_millis(500)))
            .mount(&server)
            .await;
        server
    }

    fn deadlines(server: &MockServer) -> OllamaEmbeddingConfig {
        config(server)
            .with_short_prompt_timeout(Duration::from_millis(100))
            .with_timeout(Duration::from_secs(2))
    }

    #[tokio::test]
    async fn a_short_prompt_gives_up_at_the_short_timeout() {
        let server = slow_server().await;
        let prompt = "é".repeat(SHORT_PROMPT_CHARACTERS - 1);

        let error = provider(deadlines(&server))
            .embed_single(&prompt)
            .await
            .unwrap_err();

        assert!(matches!(error, ProviderError::Network { .. }), "{error:?}");
    }

    #[tokio::test]
    async fn a_long_prompt_waits_for_the_long_timeout() {
        let server = slow_server().await;
        let prompt = "e".repeat(SHORT_PROMPT_CHARACTERS);

        let embedded = provider(deadlines(&server))
            .embed_single(&prompt)
            .await
            .unwrap();

        assert_eq!(embedded, vec![0.5]);
    }

    #[tokio::test]
    async fn a_refusal_keeps_its_status_and_message() {
        for status in [401, 403, 429, 500] {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path(PATH))
                .respond_with(
                    ResponseTemplate::new(status).set_body_string(
                        r#"{"error":"the input length exceeds the context length"}"#,
                    ),
                )
                .mount(&server)
                .await;

            let error = provider(config(&server))
                .embed_single("hello")
                .await
                .unwrap_err();

            assert!(
                matches!(error, ProviderError::Api { status: answered, .. } if answered == status),
                "{status}: {error:?}"
            );
            assert!(
                error.to_string().to_lowercase().contains("context length"),
                "{status}: the message was lost: {error}"
            );
        }
    }

    #[tokio::test]
    async fn a_key_echoed_in_a_refusal_is_redacted() {
        let masked = format!("{}****{}", &KEY[..6], &KEY[KEY.len() - 4..]);
        for echoed in [KEY.to_string(), masked] {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path(PATH))
                .respond_with(ResponseTemplate::new(401).set_body_string(format!(
                    r#"{{"error":"Authentication Error, Invalid proxy server token passed. Received API Key = {echoed}"}}"#
                )))
                .mount(&server)
                .await;

            let error = provider(config(&server).with_api_key(KEY))
                .embed_single("hello")
                .await
                .unwrap_err();
            let rendered = error.to_string();

            assert!(
                matches!(error, ProviderError::Api { status: 401, .. }),
                "{error:?}"
            );
            assert!(!rendered.contains(&echoed), "{rendered}");
            assert!(rendered.contains(REDACTED), "{rendered}");
            assert!(
                rendered.contains("Invalid proxy server token"),
                "{rendered}"
            );
        }
    }

    #[tokio::test]
    async fn a_short_key_never_hides_a_context_length_rejection() {
        for key in ["x", "token", "len"] {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path(PATH))
                .respond_with(
                    ResponseTemplate::new(500).set_body_string(
                        r#"{"error":"the input length exceeds the context length"}"#,
                    ),
                )
                .mount(&server)
                .await;

            let error = provider(config(&server).with_api_key(key))
                .embed_single("hello")
                .await
                .unwrap_err();
            let rendered = error.to_string();

            assert!(
                rendered.contains("the input length exceeds the context length"),
                "the key {key:?} garbled the refusal: {rendered}"
            );
        }
    }

    #[tokio::test]
    async fn a_vector_of_another_width_is_passed_through_untouched() {
        let server = answering(&[0.1, 0.2, 0.3]).await;
        let provider = provider(config(&server));

        let embedded = provider.embed_single("hello").await.unwrap();

        assert_eq!(provider.dimensions(), 768);
        assert_eq!(embedded, vec![0.1, 0.2, 0.3]);
    }

    #[tokio::test]
    async fn a_body_without_an_embedding_is_a_parse_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(PATH))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
            .mount(&server)
            .await;

        let error = provider(config(&server))
            .embed_single("hello")
            .await
            .unwrap_err();

        assert!(matches!(error, ProviderError::Parse { .. }), "{error:?}");
    }

    #[tokio::test]
    async fn an_empty_embedding_is_refused() {
        let server = answering(&[]).await;
        let provider = provider(config(&server));

        let single = provider.embed_single("hello").await.unwrap_err();
        let batch = provider.embed(&prompts(2)).await.unwrap_err();

        for error in [single, batch] {
            assert!(matches!(error, ProviderError::Parse { .. }), "{error:?}");
        }
    }

    #[test]
    fn a_base_url_that_is_not_http_is_refused() {
        for base_url in ["ftp://x", "not a url"] {
            let error = OllamaEmbeddingProvider::new(OllamaEmbeddingConfig::new(base_url, MODEL))
                .unwrap_err();

            assert!(
                matches!(error, ProviderError::Config { .. }),
                "{base_url}: {error:?}"
            );
        }
    }

    #[tokio::test]
    async fn a_trailing_slash_on_the_base_url_is_ignored() {
        let server = answering(&[0.5]).await;
        let config = OllamaEmbeddingConfig::new(format!("{}/", server.uri()), MODEL);

        let embedded = provider(config).embed_single("hello").await.unwrap();

        assert_eq!(embedded, vec![0.5]);
    }

    #[test]
    fn keep_alive_renders_as_minutes_or_seconds() {
        for (keep_alive, rendered) in [
            (Duration::from_secs(30 * 60), "30m"),
            (Duration::ZERO, "0m"),
            (Duration::from_secs(90), "90s"),
            (Duration::from_millis(1500), "1500ms"),
            (Duration::from_nanos(1500), "1500ns"),
        ] {
            assert_eq!(render_keep_alive(keep_alive), rendered, "{keep_alive:?}");
        }
    }

    #[test]
    fn the_tables_match_zone() {
        for (model, dimensions, tokens) in [
            ("qwen3-embedding", 1024, 8192),
            ("qwen3-embedding:0.6b", 1024, 8192),
            ("nomic-embed-text", 768, 8192),
            ("nomic-embed-text:latest", 768, 8192),
            ("Nomic-Embed-Text:v1.5", 768, 8192),
            ("nomic-embed-text-v1.5", 768, 8192),
            ("nomic-embed-text-v1", 768, 8192),
            ("mxbai-embed-large", 1024, 512),
            ("snowflake-arctic-embed", 1024, 512),
            ("bge-small-en", 384, 512),
            ("bge-small-en-v1.5", 384, 512),
            ("all-minilm", 384, 256),
            ("all-minilm-l6-v2", 384, 256),
            ("all-minilm:l6-v2", 384, 256),
            ("unknown-model", 768, 512),
            ("unknown-model:latest", 768, 512),
        ] {
            assert_eq!(model_dimensions(model), dimensions, "{model} dimensions");
            assert_eq!(model_maximum_input_tokens(model), tokens, "{model} tokens");
        }
    }

    #[test]
    fn the_qwen3_tables_disagree_as_zone_does() {
        assert_eq!(model_dimensions("x/qwen3-embedding"), 1024);
        assert_eq!(model_maximum_input_tokens("x/qwen3-embedding"), 512);
        assert_eq!(model_dimensions("qwen3-embedding:0.6b"), 1024);
        assert_eq!(model_maximum_input_tokens("qwen3-embedding:0.6b"), 8192);
    }

    #[test]
    fn the_provider_reports_its_model_and_limits() {
        let provider = provider(OllamaEmbeddingConfig::new(
            "http://localhost:11434",
            "all-minilm:latest",
        ));

        assert_eq!(provider.name(), "ollama");
        assert_eq!(provider.model(), "all-minilm:latest");
        assert_eq!(provider.dimensions(), 384);
        assert_eq!(provider.maximum_input_tokens(), 256);
        assert_eq!(provider.maximum_input_characters(), 512);
    }

    #[test]
    fn configured_dimensions_override_the_table() {
        let provider = provider(
            OllamaEmbeddingConfig::new("http://localhost:11434", MODEL).with_dimensions(256),
        );

        assert_eq!(provider.dimensions(), 256);
    }

    #[test]
    fn a_provider_never_prints_its_key() {
        let provider =
            provider(OllamaEmbeddingConfig::new("http://localhost:11434", MODEL).with_api_key(KEY));

        let printed = format!("{provider:?}");

        assert!(!printed.contains(KEY), "the key leaked: {printed}");
    }
}
