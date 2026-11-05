use std::time::Duration;

use abnegate_secret::SecretValue;

use crate::modality::embedding_provider::DEFAULT_MAXIMUM_INPUT_TOKENS;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// How to reach an OpenAI-compatible embeddings endpoint: LiteLLM, vLLM,
/// Ollama's `/v1`, a gateway, or OpenAI itself.
///
/// `base_url` already ends in the API version, as `LlmConfig` and
/// `HttpProvider` take it, such as `http://localhost:4000/v1` for a LiteLLM
/// proxy serving `qwen3-embedding:0.6b`; the provider posts to
/// `<base_url>/embeddings`. Printing a config never shows its key.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct OpenAiCompatibleEmbeddingConfig {
    /// The API root, including its version segment, such as
    /// `http://localhost:4000/v1`.
    pub base_url: String,
    /// The model to embed with, as the endpoint names it.
    pub model: String,
    /// The bearer token to send, or `None` (or an empty key) to send no
    /// `Authorization` header.
    pub api_key: Option<SecretValue>,
    /// The width to ask the model for, sent as `dimensions` only when set.
    pub dimensions: Option<u32>,
    /// The most tokens one input may hold, 512 unless set.
    pub maximum_input_tokens: u32,
    /// The deadline for one request, 30 seconds unless set.
    pub timeout: Duration,
}

impl OpenAiCompatibleEmbeddingConfig {
    /// Embed with `model` at `base_url`, with no key, the model's own width,
    /// 512 input tokens and a 30 second deadline.
    pub fn new(base_url: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            model: model.into(),
            api_key: None,
            dimensions: None,
            maximum_input_tokens: DEFAULT_MAXIMUM_INPUT_TOKENS,
            timeout: DEFAULT_TIMEOUT,
        }
    }

    /// Send `api_key` as a bearer token.
    pub fn with_api_key(mut self, api_key: impl Into<SecretValue>) -> Self {
        self.api_key = Some(api_key.into());
        self
    }

    /// Ask the model for embeddings `dimensions` values wide.
    pub fn with_dimensions(mut self, dimensions: u32) -> Self {
        self.dimensions = Some(dimensions);
        self
    }

    /// Report `maximum_input_tokens` as the most tokens one input may hold.
    pub fn with_maximum_input_tokens(mut self, maximum_input_tokens: u32) -> Self {
        self.maximum_input_tokens = maximum_input_tokens;
        self
    }

    /// Give up on a request after `timeout`.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}
