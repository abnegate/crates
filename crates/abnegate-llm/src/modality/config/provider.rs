use abnegate_secret::SecretValue;
use serde::{Deserialize, Serialize};

use crate::cost::CostStrategy;
use crate::modality::config::{
    AudioProviderConfig, EmbeddingProviderConfig, ImageProviderConfig, Model3DProviderConfig,
    TextProviderConfig, TranscriptionProviderConfig, VideoProviderConfig, VoiceProviderConfig,
};

const BUDGET_PREFIX: &str = "budget:";
const DEFAULT_BUDGET_USD: f64 = 10.0;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub text_provider: TextProviderConfig,
    pub image_provider: Option<ImageProviderConfig>,
    pub audio_provider: Option<AudioProviderConfig>,
    pub voice_provider: Option<VoiceProviderConfig>,
    pub video_provider: Option<VideoProviderConfig>,
    pub model3d_provider: Option<Model3DProviderConfig>,
    pub embedding_provider: Option<EmbeddingProviderConfig>,
    pub transcription_provider: Option<TranscriptionProviderConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_strategy: Option<String>,
}

impl ProviderConfig {
    /// A minimal config naming Anthropic as the text provider.
    pub fn anthropic(api_key: impl Into<SecretValue>) -> Self {
        Self::with_text_provider("anthropic", Some(api_key.into()), None)
    }

    /// A minimal config naming OpenAI as the text provider.
    pub fn openai(api_key: impl Into<SecretValue>) -> Self {
        Self::with_text_provider("openai", Some(api_key.into()), None)
    }

    /// A config naming a local Ollama instance as the text provider.
    pub fn ollama(model: impl Into<String>) -> Self {
        Self::with_text_provider("ollama", None, Some(model.into()))
    }

    fn with_text_provider(
        provider: &str,
        api_key: Option<SecretValue>,
        model: Option<String>,
    ) -> Self {
        Self {
            text_provider: TextProviderConfig {
                provider: provider.to_string(),
                api_key,
                oauth_token: None,
                model,
                base_url: None,
            },
            image_provider: None,
            audio_provider: None,
            voice_provider: None,
            video_provider: None,
            model3d_provider: None,
            embedding_provider: None,
            transcription_provider: None,
            cost_strategy: None,
        }
    }

    /// The `cost_strategy` field as a [`CostStrategy`], if one is set.
    ///
    /// An unrecognised name reads as [`CostStrategy::BestValue`] rather than
    /// failing, so a typo in a config file does not stop a run.
    pub fn parse_cost_strategy(&self) -> Option<CostStrategy> {
        self.cost_strategy.as_ref().map(|strategy| {
            match strategy.as_str() {
                "cheapest" | "cheapest-possible" => return CostStrategy::CheapestPossible,
                "best-quality" | "quality" => return CostStrategy::BestQuality,
                "best-value" | "value" => return CostStrategy::BestValue,
                "local-first" | "local" => return CostStrategy::LocalFirst,
                _ => {}
            }

            match strategy.strip_prefix(BUDGET_PREFIX) {
                Some(budget) => CostStrategy::Budget {
                    max_usd: budget.parse().unwrap_or(DEFAULT_BUDGET_USD),
                },
                None => CostStrategy::BestValue,
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anthropic_names_the_provider_and_holds_the_key() {
        let config = ProviderConfig::anthropic("test-key");
        assert_eq!(config.text_provider.provider, "anthropic");
        assert_eq!(
            config
                .text_provider
                .api_key
                .as_ref()
                .map(SecretValue::expose),
            Some("test-key")
        );
        assert!(config.image_provider.is_none());
    }

    #[test]
    fn openai_names_the_provider_and_holds_the_key() {
        let config = ProviderConfig::openai("sk-openai-test");
        assert_eq!(config.text_provider.provider, "openai");
        assert_eq!(
            config
                .text_provider
                .api_key
                .as_ref()
                .map(SecretValue::expose),
            Some("sk-openai-test")
        );
    }

    #[test]
    fn ollama_needs_a_model_and_no_key() {
        let config = ProviderConfig::ollama("mistral");
        assert_eq!(config.text_provider.provider, "ollama");
        assert_eq!(config.text_provider.model.as_deref(), Some("mistral"));
        assert!(config.text_provider.api_key.is_none());
    }

    #[test]
    fn a_key_never_survives_a_round_trip_through_json() {
        let config = ProviderConfig::anthropic("sk-test-key");

        let json = serde_json::to_string(&config).unwrap();
        let roundtrip: ProviderConfig = serde_json::from_str(&json).unwrap();

        assert!(!json.contains("sk-test-key"));
        assert_eq!(roundtrip.text_provider.provider, "anthropic");
        assert!(roundtrip.text_provider.api_key.is_none());
        assert!(roundtrip.image_provider.is_none());
        assert!(roundtrip.audio_provider.is_none());
        assert!(roundtrip.voice_provider.is_none());
        assert!(roundtrip.video_provider.is_none());
        assert!(roundtrip.model3d_provider.is_none());
        assert!(roundtrip.embedding_provider.is_none());
        assert!(roundtrip.transcription_provider.is_none());
        assert_eq!(json, serde_json::to_string(&roundtrip).unwrap());
    }

    #[test]
    fn a_key_never_reaches_a_debug_line() {
        let config = ProviderConfig::anthropic("sk-test-key");
        let rendered = format!("{config:?}");
        assert!(!rendered.contains("sk-test-key"), "{rendered}");
        assert!(rendered.contains("[REDACTED]"), "{rendered}");
    }

    #[test]
    fn an_oauth_token_never_reaches_a_debug_line() {
        let mut config = ProviderConfig::anthropic("sk-test-key");
        config.text_provider.oauth_token = Some(SecretValue::new("oauth-token-123"));
        let rendered = format!("{config:?}");
        assert!(!rendered.contains("oauth-token-123"), "{rendered}");
    }

    #[test]
    fn each_named_strategy_parses() {
        let cases = [
            ("cheapest", CostStrategy::CheapestPossible),
            ("cheapest-possible", CostStrategy::CheapestPossible),
            ("best-quality", CostStrategy::BestQuality),
            ("quality", CostStrategy::BestQuality),
            ("best-value", CostStrategy::BestValue),
            ("value", CostStrategy::BestValue),
            ("local-first", CostStrategy::LocalFirst),
            ("local", CostStrategy::LocalFirst),
        ];

        for (name, expected) in cases {
            let mut config = ProviderConfig::anthropic("key");
            config.cost_strategy = Some(name.into());
            assert_eq!(config.parse_cost_strategy(), Some(expected), "{name}");
        }
    }

    #[test]
    fn a_budget_strategy_carries_its_limit() {
        let mut config = ProviderConfig::anthropic("key");
        config.cost_strategy = Some("budget:25.50".into());

        match config.parse_cost_strategy().unwrap() {
            CostStrategy::Budget { max_usd } => {
                assert!((max_usd - 25.50).abs() < f64::EPSILON);
            }
            other => panic!("expected Budget, got {other:?}"),
        }
    }

    #[test]
    fn an_unparseable_budget_falls_back_to_the_default() {
        let mut config = ProviderConfig::anthropic("key");
        config.cost_strategy = Some("budget:lots".into());

        match config.parse_cost_strategy().unwrap() {
            CostStrategy::Budget { max_usd } => {
                assert!((max_usd - DEFAULT_BUDGET_USD).abs() < f64::EPSILON);
            }
            other => panic!("expected Budget, got {other:?}"),
        }
    }

    #[test]
    fn no_strategy_parses_to_nothing() {
        assert!(
            ProviderConfig::anthropic("key")
                .parse_cost_strategy()
                .is_none()
        );
    }

    #[test]
    fn an_unknown_strategy_reads_as_best_value() {
        let mut config = ProviderConfig::anthropic("key");
        config.cost_strategy = Some("unknown-strategy".into());
        assert_eq!(config.parse_cost_strategy(), Some(CostStrategy::BestValue));
    }
}
