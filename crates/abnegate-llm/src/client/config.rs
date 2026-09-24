use std::time::Duration;

use abnegate_secret::SecretValue;

const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";
const DEFAULT_MODEL: &str = "gpt-4";
const DEFAULT_TEMPERATURE: f32 = 0.7;
const DEFAULT_MAXIMUM_TOKENS: u32 = 4096;
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const DEFAULT_READ_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// How an [`LlmClient`](crate::LlmClient) reaches its endpoint.
///
/// The key is a [`SecretValue`], so this config can be printed, and the
/// client that embeds it logged, without the key reaching the output.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct LlmConfig {
    /// Base URL for the API, such as `https://api.openai.com/v1`.
    pub base_url: String,
    /// Sent as a bearer token. An empty key sends no `Authorization` header.
    pub api_key: SecretValue,
    pub default_model: String,
    pub temperature: f32,
    /// The most tokens an answer may use, for a request that does not reserve
    /// its own through [`RequestOptions`](crate::RequestOptions). Sent as
    /// `max_tokens`.
    pub maximum_tokens: u32,
    /// The deadline for a whole non-streaming completion, from sending the
    /// request to reading the last byte of its body.
    pub timeout: Duration,
    /// The longest a streamed completion may go without a byte, including the
    /// wait for the first. A stream as a whole has no deadline.
    pub read_timeout: Duration,
}

impl LlmConfig {
    /// A config for `base_url` that asks `default_model` and authenticates
    /// with `api_key`, with every other setting at its default.
    pub fn new(
        base_url: impl Into<String>,
        default_model: impl Into<String>,
        api_key: impl Into<SecretValue>,
    ) -> Self {
        Self {
            base_url: base_url.into(),
            api_key: api_key.into(),
            default_model: default_model.into(),
            ..Self::default()
        }
    }

    pub fn with_temperature(mut self, temperature: f32) -> Self {
        self.temperature = temperature;
        self
    }

    /// Set [`Self::maximum_tokens`].
    pub fn with_maximum_tokens(mut self, maximum_tokens: u32) -> Self {
        self.maximum_tokens = maximum_tokens;
        self
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn with_read_timeout(mut self, read_timeout: Duration) -> Self {
        self.read_timeout = read_timeout;
        self
    }
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            base_url: DEFAULT_BASE_URL.to_string(),
            api_key: SecretValue::new(""),
            default_model: DEFAULT_MODEL.to_string(),
            temperature: DEFAULT_TEMPERATURE,
            maximum_tokens: DEFAULT_MAXIMUM_TOKENS,
            timeout: DEFAULT_TIMEOUT,
            read_timeout: DEFAULT_READ_TIMEOUT,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::LlmConfig;

    #[test]
    fn a_default_config_points_at_openai() {
        let config = LlmConfig::default();

        assert_eq!(config.base_url, "https://api.openai.com/v1");
        assert!(config.api_key.is_empty());
        assert_eq!(config.default_model, "gpt-4");
        assert!((config.temperature - 0.7).abs() < f32::EPSILON);
        assert_eq!(config.maximum_tokens, 4096);
        assert_eq!(config.timeout, Duration::from_secs(600));
        assert_eq!(config.read_timeout, Duration::from_secs(300));
    }

    #[test]
    fn a_config_keeps_every_setting_it_was_given() {
        let config = LlmConfig::new(
            "https://custom.api.com/v1",
            "gpt-3.5-turbo",
            "sk-test-key-123",
        )
        .with_temperature(0.5)
        .with_maximum_tokens(2048)
        .with_timeout(Duration::from_secs(30))
        .with_read_timeout(Duration::from_secs(5));

        assert_eq!(config.base_url, "https://custom.api.com/v1");
        assert_eq!(config.api_key.expose(), "sk-test-key-123");
        assert_eq!(config.default_model, "gpt-3.5-turbo");
        assert!((config.temperature - 0.5).abs() < f32::EPSILON);
        assert_eq!(config.maximum_tokens, 2048);
        assert_eq!(config.timeout, Duration::from_secs(30));
        assert_eq!(config.read_timeout, Duration::from_secs(5));
    }

    #[test]
    fn a_cloned_config_matches_its_original() {
        let config = LlmConfig::new("https://test.api.com", "test-model", "test-key");

        let cloned = config.clone();
        assert_eq!(cloned.base_url, config.base_url);
        assert_eq!(cloned.api_key, config.api_key);
        assert_eq!(cloned.default_model, config.default_model);
    }

    /// A default config's key is empty, so asserting that its debug output
    /// merely *mentions* `api_key` would be satisfied by any `Debug`. The key
    /// here is a real one, and what is asserted is that its value is absent.
    #[test]
    fn a_config_never_prints_its_key() {
        const KEY: &str = "sk-test-3f8a1c9e04b27d65";
        let config = LlmConfig::new("https://api.openai.com/v1", "gpt-4", KEY);
        let debug = format!("{config:?}");

        assert!(debug.contains("LlmConfig"));
        assert!(debug.contains("base_url"));
        assert!(debug.contains("default_model"));
        assert!(
            debug.contains("api_key"),
            "the field should still be named, so its redaction is visible: {debug}"
        );
        assert!(
            !debug.contains(KEY),
            "the provider key reached a debug line: {debug}"
        );
    }
}
