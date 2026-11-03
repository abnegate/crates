#[cfg(any(feature = "anthropic", feature = "openai"))]
use abnegate_secret::SecretValue;
use serde::Deserialize;
use serde::Serialize;

use crate::cost::CostStrategy;
use crate::cost::strategy::BUDGET_PREFIX;
use crate::modality::config::AudioProviderConfig;
use crate::modality::config::EmbeddingProviderConfig;
use crate::modality::config::ImageProviderConfig;
use crate::modality::config::Model3DProviderConfig;
use crate::modality::config::TextProviderConfig;
use crate::modality::config::TranscriptionProviderConfig;
use crate::modality::config::VideoProviderConfig;
use crate::modality::config::VoiceProviderConfig;
use crate::parse_error::ParseError;

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
#[non_exhaustive]
pub struct ProviderConfig {
    /// Text generation, the one modality every config has.
    pub text_provider: TextProviderConfig,
    /// Image generation, when configured.
    pub image_provider: Option<ImageProviderConfig>,
    /// Music and sound effects, when configured.
    pub audio_provider: Option<AudioProviderConfig>,
    /// Speech synthesis, when configured.
    pub voice_provider: Option<VoiceProviderConfig>,
    /// Video generation, when configured.
    pub video_provider: Option<VideoProviderConfig>,
    /// 3D model generation, when configured.
    pub model3d_provider: Option<Model3DProviderConfig>,
    /// Embeddings, when configured.
    pub embedding_provider: Option<EmbeddingProviderConfig>,
    /// Speech to text, when configured.
    pub transcription_provider: Option<TranscriptionProviderConfig>,
    /// The [`CostStrategy`] as a config file spells it, such as
    /// `best-value` or `budget:10`. [`cost_strategy()`](Self::cost_strategy())
    /// is the strict reader, which reports a typo instead of guessing.
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

    /// Set [`Self::image_provider`].
    pub fn with_image_provider(mut self, image_provider: ImageProviderConfig) -> Self {
        self.image_provider = Some(image_provider);
        self
    }

    /// Set [`Self::audio_provider`].
    pub fn with_audio_provider(mut self, audio_provider: AudioProviderConfig) -> Self {
        self.audio_provider = Some(audio_provider);
        self
    }

    /// Set [`Self::voice_provider`].
    pub fn with_voice_provider(mut self, voice_provider: VoiceProviderConfig) -> Self {
        self.voice_provider = Some(voice_provider);
        self
    }

    /// Set [`Self::video_provider`].
    pub fn with_video_provider(mut self, video_provider: VideoProviderConfig) -> Self {
        self.video_provider = Some(video_provider);
        self
    }

    /// Set [`Self::model3d_provider`].
    pub fn with_model3d_provider(mut self, model3d_provider: Model3DProviderConfig) -> Self {
        self.model3d_provider = Some(model3d_provider);
        self
    }

    /// Set [`Self::embedding_provider`].
    pub fn with_embedding_provider(mut self, embedding_provider: EmbeddingProviderConfig) -> Self {
        self.embedding_provider = Some(embedding_provider);
        self
    }

    /// Set [`Self::transcription_provider`].
    pub fn with_transcription_provider(
        mut self,
        transcription_provider: TranscriptionProviderConfig,
    ) -> Self {
        self.transcription_provider = Some(transcription_provider);
        self
    }

    /// Name the [`CostStrategy`] as a config file spells it, such as
    /// `best-value` or `budget:10`; [`Self::cost_strategy()`] reads it.
    pub fn with_cost_strategy(mut self, cost_strategy: impl Into<String>) -> Self {
        self.cost_strategy = Some(cost_strategy.into());
        self
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

    /// The `cost_strategy` field as a [`CostStrategy`], or `None` when it is
    /// not set.
    ///
    /// # Errors
    ///
    /// [`ParseError`] when the field names no strategy, or names a budget that
    /// is not a finite amount of zero or more.
    pub fn cost_strategy(&self) -> Result<Option<CostStrategy>, ParseError> {
        self.cost_strategy.as_deref().map(str::parse).transpose()
    }

    /// The `cost_strategy` field as a [`CostStrategy`], if one is set,
    /// guessing at one it cannot read.
    ///
    /// An unrecognised name reads as [`CostStrategy::BestValue`], and a budget
    /// that is not a finite amount of zero or more reads as a ten dollar
    /// budget, so a typo silently changes what a run spends.
    /// [`Self::cost_strategy()`] reports the typo instead.
    #[deprecated(
        since = "0.1.4",
        note = "use `cost_strategy()`, which reports a malformed budget or an unknown name instead of guessing"
    )]
    pub fn parse_cost_strategy(&self) -> Option<CostStrategy> {
        self.cost_strategy()
            .unwrap_or_else(|error| Some(guessed(error.value())))
    }
}

fn guessed(strategy: &str) -> CostStrategy {
    if strategy.starts_with(BUDGET_PREFIX) {
        CostStrategy::budget(DEFAULT_BUDGET_USD)
    } else {
        CostStrategy::BestValue
    }
}

#[cfg(test)]
mod tests {
    use abnegate_secret::SecretValue;

    use super::*;
    use crate::cost::strategy::KIND;

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
    fn every_modality_it_names_survives_a_round_trip_without_its_key() {
        let config = keyed("openai", "sk-text")
            .with_image_provider(
                ImageProviderConfig::new("openai")
                    .with_api_key("sk-image")
                    .with_model("gpt-image-1"),
            )
            .with_audio_provider(AudioProviderConfig::new("elevenlabs").with_api_key("sk-audio"))
            .with_voice_provider(VoiceProviderConfig::new("elevenlabs").with_api_key("sk-voice"))
            .with_video_provider(
                VideoProviderConfig::new("runway").with_base_url("http://127.0.0.1:9000"),
            )
            .with_model3d_provider(Model3DProviderConfig::new("meshy"))
            .with_embedding_provider(EmbeddingProviderConfig::new("openai").with_model("small"))
            .with_transcription_provider(TranscriptionProviderConfig::new("openai"))
            .with_cost_strategy("budget:10");

        let json = serde_json::to_string(&config).unwrap();
        let roundtrip: ProviderConfig = serde_json::from_str(&json).unwrap();

        assert!(!json.contains("sk-"), "{json}");
        assert_eq!(
            roundtrip.cost_strategy(),
            Ok(Some(CostStrategy::budget(10.0)))
        );
        let image = roundtrip.image_provider.unwrap();
        assert_eq!(image.provider, "openai");
        assert_eq!(image.model.as_deref(), Some("gpt-image-1"));
        assert!(image.api_key.is_none());
        assert_eq!(roundtrip.audio_provider.unwrap().provider, "elevenlabs");
        assert_eq!(roundtrip.voice_provider.unwrap().provider, "elevenlabs");
        assert_eq!(
            roundtrip.video_provider.unwrap().base_url.as_deref(),
            Some("http://127.0.0.1:9000")
        );
        assert_eq!(roundtrip.model3d_provider.unwrap().provider, "meshy");
        assert_eq!(
            roundtrip.embedding_provider.unwrap().model.as_deref(),
            Some("small")
        );
        assert_eq!(roundtrip.transcription_provider.unwrap().provider, "openai");
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
            ("budget:25.50", CostStrategy::budget(25.5)),
            ("budget:0", CostStrategy::budget(0.0)),
        ];

        for (name, expected) in cases {
            let config = keyed("anthropic", "key").with_cost_strategy(name);
            assert_eq!(config.cost_strategy(), Ok(Some(expected)), "{name}");
        }
    }

    #[test]
    fn no_strategy_parses_to_nothing() {
        assert_eq!(keyed("anthropic", "key").cost_strategy(), Ok(None));
    }

    #[test]
    fn a_malformed_budget_is_refused() {
        for budget in [
            "budget:lots",
            "budget:NaN",
            "budget:-5",
            "budget:inf",
            "budget:",
        ] {
            let config = keyed("anthropic", "key").with_cost_strategy(budget);

            let error = config.cost_strategy().unwrap_err();

            assert_eq!(error.kind(), KIND, "{budget}");
            assert_eq!(error.value(), budget);
        }
    }

    #[test]
    fn an_unknown_strategy_is_refused() {
        let config = keyed("anthropic", "key").with_cost_strategy("unknown-strategy");

        let error = config.cost_strategy().unwrap_err();

        assert_eq!(error.kind(), KIND);
        assert_eq!(error.value(), "unknown-strategy");
    }

    #[test]
    #[expect(deprecated)]
    fn the_lenient_reading_agrees_with_the_strict_one_on_what_it_can_read() {
        for name in [
            "cheapest",
            "best-quality",
            "best-value",
            "local",
            "budget:25.50",
        ] {
            let config = keyed("anthropic", "key").with_cost_strategy(name);
            assert_eq!(
                config.parse_cost_strategy(),
                config.cost_strategy().unwrap(),
                "{name}"
            );
        }
        assert!(keyed("anthropic", "key").parse_cost_strategy().is_none());
    }

    #[test]
    #[expect(deprecated)]
    fn the_lenient_reading_takes_a_malformed_budget_as_the_default_budget() {
        for budget in [
            "budget:lots",
            "budget:NaN",
            "budget:-5",
            "budget:inf",
            "budget:",
        ] {
            let config = keyed("anthropic", "key").with_cost_strategy(budget);

            assert_eq!(
                config.parse_cost_strategy(),
                Some(CostStrategy::budget(DEFAULT_BUDGET_USD)),
                "{budget}"
            );
        }
    }

    #[test]
    #[expect(deprecated)]
    fn the_lenient_reading_takes_an_unknown_name_as_best_value() {
        let config = keyed("anthropic", "key").with_cost_strategy("unknown-strategy");
        assert_eq!(config.parse_cost_strategy(), Some(CostStrategy::BestValue));
    }
}
