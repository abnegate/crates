use serde::Deserialize;
use serde::Serialize;

/// How an [`Agent`](super::Agent) runs a turn.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct AgentConfig {
    /// Model rounds one turn may spend before it fails.
    #[serde(rename = "max_iterations")]
    pub maximum_iterations: usize,
    /// Tokens reserved for each reply, both in the request and in the context
    /// budget compaction works to.
    #[serde(rename = "max_tokens")]
    pub maximum_tokens: u32,
    /// Sampling temperature for this agent's requests, or the provider's own
    /// when unset.
    #[serde(default)]
    pub temperature: Option<f32>,
    /// The system prompt, or the default one listing the tools when unset.
    pub system_prompt: Option<String>,
}

impl AgentConfig {
    /// The same config, allowing `maximum_iterations` model rounds a turn.
    ///
    /// The config is non-exhaustive, so a caller outside this crate starts
    /// from [`Default`] and changes what it needs:
    ///
    /// ```
    /// use abnegate_agent::AgentConfig;
    ///
    /// let config = AgentConfig::default()
    ///     .with_maximum_iterations(10)
    ///     .with_system_prompt("You review pull requests.");
    /// assert_eq!(config.maximum_iterations, 10);
    /// ```
    pub fn with_maximum_iterations(mut self, maximum_iterations: usize) -> Self {
        self.maximum_iterations = maximum_iterations;
        self
    }

    /// The same config, reserving `maximum_tokens` for each reply.
    pub fn with_maximum_tokens(mut self, maximum_tokens: u32) -> Self {
        self.maximum_tokens = maximum_tokens;
        self
    }

    /// The same config, sampling at `temperature`.
    pub fn with_temperature(mut self, temperature: f32) -> Self {
        self.temperature = Some(temperature);
        self
    }

    /// The same config, with its own system prompt in place of the default.
    pub fn with_system_prompt(mut self, system_prompt: impl Into<String>) -> Self {
        self.system_prompt = Some(system_prompt.into());
        self
    }
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            maximum_iterations: 50,
            maximum_tokens: 4096,
            temperature: None,
            system_prompt: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn test_agent_config_default() {
        let config = AgentConfig::default();
        assert_eq!(config.maximum_iterations, 50);
        assert_eq!(config.maximum_tokens, 4096);
        assert_eq!(config.temperature, None);
        assert!(config.system_prompt.is_none());
    }

    #[test]
    fn every_setting_is_changed_through_its_builder() {
        let config = AgentConfig::default()
            .with_maximum_iterations(75)
            .with_maximum_tokens(2048)
            .with_temperature(0.5)
            .with_system_prompt("You are a coding assistant");

        assert_eq!(config.maximum_iterations, 75);
        assert_eq!(config.maximum_tokens, 2048);
        assert_eq!(config.temperature, Some(0.5));
        assert_eq!(
            config.system_prompt.as_deref(),
            Some("You are a coding assistant")
        );
    }

    #[test]
    fn test_agent_config_serialization_roundtrip() {
        let config = AgentConfig::default()
            .with_maximum_iterations(75)
            .with_maximum_tokens(2048)
            .with_temperature(0.5)
            .with_system_prompt("You are a coding assistant");

        let json = serde_json::to_string(&config).unwrap();
        let deserialized: AgentConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.maximum_iterations, config.maximum_iterations);
        assert_eq!(deserialized.maximum_tokens, config.maximum_tokens);
        assert_eq!(deserialized.temperature, Some(0.5));
        assert_eq!(deserialized.system_prompt, config.system_prompt);
    }

    /// Spelling the fields out must not change the keys a saved config is
    /// read from or written to.
    #[test]
    fn a_config_saved_under_the_abbreviated_keys_still_reads_and_writes_them() {
        let saved = json!({
            "max_iterations": 12,
            "max_tokens": 777,
            "temperature": 0.25,
            "system_prompt": "Be brief."
        });

        let config: AgentConfig = serde_json::from_value(saved.clone()).unwrap();

        assert_eq!(config.maximum_iterations, 12);
        assert_eq!(config.maximum_tokens, 777);
        assert_eq!(config.temperature, Some(0.25));
        assert_eq!(config.system_prompt.as_deref(), Some("Be brief."));
        assert_eq!(serde_json::to_value(&config).unwrap(), saved);
    }

    #[test]
    fn a_config_saved_before_temperature_was_optional_still_reads() {
        let config: AgentConfig = serde_json::from_str(
            r#"{"max_iterations": 5, "max_tokens": 10, "stream": false, "system_prompt": null}"#,
        )
        .unwrap();
        assert_eq!(config.maximum_iterations, 5);
        assert_eq!(config.maximum_tokens, 10);
        assert_eq!(config.temperature, None);
    }
}
