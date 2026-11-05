use std::time::Duration;

use abnegate_secret::SecretValue;

const DEFAULT_CONCURRENCY: usize = 10;
const DEFAULT_KEEP_ALIVE: Duration = Duration::from_secs(30 * 60);
const DEFAULT_SHORT_PROMPT_TIMEOUT: Duration = Duration::from_secs(8);
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// How to reach an Ollama server's native embeddings API.
///
/// `Debug` prints the key as `[REDACTED]`.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct OllamaEmbeddingConfig {
    /// The server's root, such as `http://localhost:11434`; a trailing `/`
    /// is ignored.
    pub base_url: String,
    /// The model to embed with, such as `nomic-embed-text` or
    /// `qwen3-embedding:0.6b`.
    pub model: String,
    /// A key sent as a bearer token, for a server behind a proxy that asks
    /// for one. The key is sent trimmed, and `None` or a blank key sends no
    /// `Authorization` header.
    pub api_key: Option<SecretValue>,
    /// The width each embedding is expected to have, or `None` for the width
    /// Ollama's own build of the model gives.
    pub dimensions: Option<u32>,
    /// How long Ollama keeps the model loaded after a request, or `None` to
    /// leave it to the server; 30 minutes by default.
    pub keep_alive: Option<Duration>,
    /// The deadline for a prompt of 2,000 characters or more; 30 seconds by
    /// default.
    pub timeout: Duration,
    /// The deadline for a prompt under 2,000 characters; 8 seconds by
    /// default.
    pub short_prompt_timeout: Duration,
    /// How many prompts of one batch are in flight at once; 10 by default.
    /// Values below 1 become 1.
    pub concurrency: usize,
}

impl OllamaEmbeddingConfig {
    /// Embed with `model` on the server at `base_url`, without a key, keeping
    /// the model loaded for 30 minutes and up to 10 prompts in flight.
    pub fn new(base_url: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            model: model.into(),
            api_key: None,
            dimensions: None,
            keep_alive: Some(DEFAULT_KEEP_ALIVE),
            timeout: DEFAULT_TIMEOUT,
            short_prompt_timeout: DEFAULT_SHORT_PROMPT_TIMEOUT,
            concurrency: DEFAULT_CONCURRENCY,
        }
    }

    /// Send `api_key` as a bearer token.
    pub fn with_api_key(mut self, api_key: impl Into<SecretValue>) -> Self {
        self.api_key = Some(api_key.into());
        self
    }

    /// Expect embeddings `dimensions` wide.
    pub fn with_dimensions(mut self, dimensions: u32) -> Self {
        self.dimensions = Some(dimensions);
        self
    }

    /// Ask Ollama to keep the model loaded for `keep_alive`.
    pub fn with_keep_alive(mut self, keep_alive: Duration) -> Self {
        self.keep_alive = Some(keep_alive);
        self
    }

    /// Give up on a prompt of 2,000 characters or more after `timeout`.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Give up on a prompt under 2,000 characters after
    /// `short_prompt_timeout`.
    pub fn with_short_prompt_timeout(mut self, short_prompt_timeout: Duration) -> Self {
        self.short_prompt_timeout = short_prompt_timeout;
        self
    }

    /// Keep up to `concurrency` prompts of one batch in flight.
    pub fn with_concurrency(mut self, concurrency: usize) -> Self {
        self.concurrency = concurrency;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = concat!("sk-", "ollama-proxy-0123456789abcdef");

    #[test]
    fn a_new_config_matches_what_zone_sends() {
        let config = OllamaEmbeddingConfig::new("http://localhost:11434", "nomic-embed-text");

        assert_eq!(config.base_url, "http://localhost:11434");
        assert_eq!(config.model, "nomic-embed-text");
        assert!(config.api_key.is_none());
        assert_eq!(config.dimensions, None);
        assert_eq!(config.keep_alive, Some(Duration::from_secs(1800)));
        assert_eq!(config.timeout, Duration::from_secs(30));
        assert_eq!(config.short_prompt_timeout, Duration::from_secs(8));
        assert_eq!(config.concurrency, 10);
    }

    #[test]
    fn builders_replace_the_fields_they_name() {
        let config = OllamaEmbeddingConfig::new("http://localhost:11434", "nomic-embed-text")
            .with_api_key(KEY)
            .with_dimensions(384)
            .with_keep_alive(Duration::from_secs(90))
            .with_timeout(Duration::from_secs(60))
            .with_short_prompt_timeout(Duration::from_secs(2))
            .with_concurrency(4);

        assert_eq!(config.api_key.as_ref().map(SecretValue::expose), Some(KEY));
        assert_eq!(config.dimensions, Some(384));
        assert_eq!(config.keep_alive, Some(Duration::from_secs(90)));
        assert_eq!(config.timeout, Duration::from_secs(60));
        assert_eq!(config.short_prompt_timeout, Duration::from_secs(2));
        assert_eq!(config.concurrency, 4);
    }

    #[test]
    fn a_config_never_prints_its_key() {
        let config = OllamaEmbeddingConfig::new("http://localhost:11434", "nomic-embed-text")
            .with_api_key(KEY);

        let printed = format!("{config:?}");

        assert!(!printed.contains(KEY), "the key leaked: {printed}");
        assert!(printed.contains("[REDACTED]"), "{printed}");
    }
}
