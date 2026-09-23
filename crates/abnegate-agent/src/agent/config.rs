use serde::{Deserialize, Serialize};

/// How an [`Agent`](super::Agent) runs a turn.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    /// Model rounds one turn may spend before it fails.
    pub max_iterations: usize,
    /// Tokens reserved for each reply, both in the request and in the context
    /// budget compaction works to.
    pub max_tokens: u32,
    /// Sampling temperature for this agent's requests, or the client's own
    /// when unset.
    #[serde(default)]
    pub temperature: Option<f32>,
    /// The system prompt, or the default one listing the tools when unset.
    pub system_prompt: Option<String>,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            max_iterations: 50,
            max_tokens: 4096,
            temperature: None,
            system_prompt: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agent_config_default() {
        let config = AgentConfig::default();
        assert_eq!(config.max_iterations, 50);
        assert_eq!(config.max_tokens, 4096);
        assert_eq!(config.temperature, None);
        assert!(config.system_prompt.is_none());
    }

    #[test]
    fn test_agent_config_serialization_roundtrip() {
        let config = AgentConfig {
            max_iterations: 75,
            max_tokens: 2048,
            temperature: Some(0.5),
            system_prompt: Some("You are a coding assistant".to_string()),
        };

        let json = serde_json::to_string(&config).unwrap();
        let deserialized: AgentConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.max_iterations, config.max_iterations);
        assert_eq!(deserialized.max_tokens, config.max_tokens);
        assert_eq!(deserialized.temperature, Some(0.5));
        assert_eq!(deserialized.system_prompt, config.system_prompt);
    }

    #[test]
    fn a_config_saved_before_temperature_was_optional_still_reads() {
        let config: AgentConfig = serde_json::from_str(
            r#"{"max_iterations": 5, "max_tokens": 10, "stream": false, "system_prompt": null}"#,
        )
        .unwrap();
        assert_eq!(config.temperature, None);
    }
}
