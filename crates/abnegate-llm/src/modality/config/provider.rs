#[cfg(any(feature = "anthropic", feature = "openai"))]
use abnegate_secret::SecretValue;
use serde::{Deserialize, Serialize};

use crate::cost::CostStrategy;
use crate::modality::config::{
    AudioProviderConfig, EmbeddingProviderConfig, ImageProviderConfig, Model3DProviderConfig,
    TextProviderConfig, TranscriptionProviderConfig, VideoProviderConfig, VoiceProviderConfig,
};

const BUDGET_PREFIX: &str = "budget:";
const DEFAULT_BUDGET_USD: f64 = 10.0;

/// Which provider serves each modality, as a configuration file names them.
///
/// Configuration only: nothing in this crate builds a provider from it. The
/// names are the caller's to resolve, and the vendor clients this crate ships
/// are the ones in [`vendor`](crate::modality::vendor) — Anthropic, Gemini and
/// OpenAI, each behind its feature — plus any OpenAI-compatible endpoint
/// through [`HttpProvider`](crate::HttpProvider) and
/// [`CompletionBridge`](crate::modality::CompletionBridge). A name outside
/// that set describes a provider the caller supplies.
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
    /// A config with `text_provider` and no other modality configured.
    pub fn new(text_provider: TextProviderConfig) -> Self {
        Self {
            text_provider,
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

    /// A minimal config naming Anthropic as the text provider.
    #[cfg(feature = "anthropic")]
    #[cfg_attr(docsrs, doc(cfg(feature = "anthropic")))]
    pub fn anthropic(api_key: impl Into<SecretValue>) -> Self {
        Self::new(TextProviderConfig::new("anthropic").with_api_key(api_key))
    }

    /// A minimal config naming OpenAI as the text provider.
    #[cfg(feature = "openai")]
    #[cfg_attr(docsrs, doc(cfg(feature = "openai")))]
    pub fn openai(api_key: impl Into<SecretValue>) -> Self {
        Self::new(TextProviderConfig::new("openai").with_api_key(api_key))
    }

    /// The `cost_strategy` field as a [`CostStrategy`], if one is set.
    ///
    /// Leniently: an unrecognised name reads as [`CostStrategy::BestValue`],
    /// and a budget that is not a finite amount of zero or more reads as a
    /// ten dollar budget, so a typo in a config file does not stop a run. Parse
    /// the field with [`str::parse`] to be told about the typo instead.
    pub fn parse_cost_strategy(&self) -> Option<CostStrategy> {
        self.cost_strategy.as_deref().map(|strategy| {
            strategy.parse().unwrap_or_else(|_| {
                if strategy.starts_with(BUDGET_PREFIX) {
                    CostStrategy::Budget {
                        max_usd: DEFAULT_BUDGET_USD,
                    }
                } else {
                    CostStrategy::BestValue
                }
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use abnegate_secret::SecretValue;

    use super::*;

    fn keyed(provider: &str, key: &str) -> ProviderConfig {
        ProviderConfig::new(TextProviderConfig::new(provider).with_api_key(key))
    }

    #[test]
    fn anthropic_names_the_provider_and_holds_the_key() {
        let config = keyed("anthropic", "test-key");
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
        let config = keyed("openai", "sk-openai-test");
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
    fn a_keyless_provider_names_its_model_and_holds_no_key() {
        let config = ProviderConfig::new(
            TextProviderConfig::new("ollama")
                .with_model("mistral")
                .with_base_url("http://127.0.0.1:11434/v1"),
        );
        assert_eq!(config.text_provider.provider, "ollama");
        assert_eq!(config.text_provider.model.as_deref(), Some("mistral"));
        assert!(config.text_provider.api_key.is_none());
    }

    #[cfg(feature = "anthropic")]
    #[test]
    fn the_anthropic_shorthand_names_the_provider_and_holds_the_key() {
        let config = ProviderConfig::anthropic(concat!("sk-ant-", "test"));
        assert_eq!(config.text_provider.provider, "anthropic");
        assert_eq!(
            config
                .text_provider
                .api_key
                .as_ref()
                .map(SecretValue::expose),
            Some(concat!("sk-ant-", "test"))
        );
    }

    #[cfg(feature = "openai")]
    #[test]
    fn the_openai_shorthand_names_the_provider_and_holds_the_key() {
        let config = ProviderConfig::openai("sk-openai-test");
        assert_eq!(config.text_provider.provider, "openai");
    }

    #[test]
    fn a_key_never_survives_a_round_trip_through_json() {
        let config = keyed("anthropic", "sk-test-key");

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
        let config = keyed("anthropic", "sk-test-key");
        let rendered = format!("{config:?}");
        assert!(!rendered.contains("sk-test-key"), "{rendered}");
        assert!(rendered.contains("[REDACTED]"), "{rendered}");
    }

    #[test]
    fn an_oauth_token_never_reaches_a_debug_line() {
        let mut config = keyed("anthropic", "sk-test-key");
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
            let mut config = keyed("anthropic", "key");
            config.cost_strategy = Some(name.into());
            assert_eq!(config.parse_cost_strategy(), Some(expected), "{name}");
        }
    }

    #[test]
    fn a_budget_strategy_carries_its_limit() {
        let mut config = keyed("anthropic", "key");
        config.cost_strategy = Some("budget:25.50".into());

        match config.parse_cost_strategy().unwrap() {
            CostStrategy::Budget { max_usd } => {
                assert!((max_usd - 25.50).abs() < f64::EPSILON);
            }
            other => panic!("expected Budget, got {other:?}"),
        }
    }

    #[test]
    fn a_budget_that_is_not_a_finite_amount_falls_back_to_the_default() {
        for budget in ["budget:NaN", "budget:-5", "budget:inf"] {
            let mut config = keyed("anthropic", "key");
            config.cost_strategy = Some(budget.into());

            assert_eq!(
                config.parse_cost_strategy(),
                Some(CostStrategy::Budget {
                    max_usd: DEFAULT_BUDGET_USD
                }),
                "{budget}"
            );
        }
    }

    #[test]
    fn an_unparseable_budget_falls_back_to_the_default() {
        let mut config = keyed("anthropic", "key");
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
        assert!(keyed("anthropic", "key").parse_cost_strategy().is_none());
    }

    #[test]
    fn an_unknown_strategy_reads_as_best_value() {
        let mut config = keyed("anthropic", "key");
        config.cost_strategy = Some("unknown-strategy".into());
        assert_eq!(config.parse_cost_strategy(), Some(CostStrategy::BestValue));
    }
}
