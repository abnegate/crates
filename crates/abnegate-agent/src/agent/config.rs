use serde::{Deserialize, Serialize};

/// Configuration for the agent
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    /// Maximum number of iterations before stopping
    pub max_iterations: usize,
    /// Maximum total tokens to use
    pub max_tokens: u32,
    /// Temperature for LLM calls
    pub temperature: f32,
    /// Whether to stream responses
    pub stream: bool,
    /// System prompt to use
    pub system_prompt: Option<String>,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            max_iterations: 50,
            max_tokens: 4096,
            temperature: 0.7,
            stream: false,
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
        assert_eq!(config.temperature, 0.7);
        assert!(!config.stream);
        assert!(config.system_prompt.is_none());
    }

    #[test]
    fn test_agent_config_custom() {
        let config = AgentConfig {
            max_iterations: 100,
            max_tokens: 8192,
            temperature: 0.3,
            stream: true,
            system_prompt: Some("Custom system prompt".to_string()),
        };

        assert_eq!(config.max_iterations, 100);
        assert_eq!(config.max_tokens, 8192);
        assert!((config.temperature - 0.3).abs() < f32::EPSILON);
        assert!(config.stream);
        assert_eq!(
            config.system_prompt,
            Some("Custom system prompt".to_string())
        );
    }

    #[test]
    fn test_agent_config_serialization_roundtrip() {
        let config = AgentConfig {
            max_iterations: 75,
            max_tokens: 2048,
            temperature: 0.5,
            stream: false,
            system_prompt: Some("You are a coding assistant".to_string()),
        };

        let json = serde_json::to_string(&config).unwrap();
        let deserialized: AgentConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.max_iterations, config.max_iterations);
        assert_eq!(deserialized.max_tokens, config.max_tokens);
        assert_eq!(deserialized.system_prompt, config.system_prompt);
    }

    #[test]
    fn test_agent_config_clone() {
        let config = AgentConfig::default();
        let cloned = config.clone();

        assert_eq!(cloned.max_iterations, config.max_iterations);
        assert_eq!(cloned.max_tokens, config.max_tokens);
    }
}
