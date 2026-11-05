use std::path::PathBuf;

use fastembed::EmbeddingModel;
use fastembed::QuantizationMode;
use fastembed::TextEmbedding;

use crate::modality::embedding_provider::DEFAULT_MAXIMUM_INPUT_TOKENS;
use crate::provider::ProviderError;

const DEFAULT_MODEL: &str = "nomic-embed-text-v1.5";
const FAST_MODEL: &str = "all-minilm-l6-v2";
const TAG_SEPARATOR: char = ':';
const NOMIC_MAXIMUM_INPUT_TOKENS: u32 = 2048;
const MINILM_MAXIMUM_INPUT_TOKENS: u32 = 256;
const BGE_M3_MAXIMUM_INPUT_TOKENS: u32 = 8192;
const SNOWFLAKE_ARCTIC_EMBED: &str = "snowflake-arctic-embed";
const SNOWFLAKE_ARCTIC_EMBED_SIZES: [(&str, EmbeddingModel); 5] = [
    ("22m", EmbeddingModel::SnowflakeArcticEmbedXS),
    ("33m", EmbeddingModel::SnowflakeArcticEmbedS),
    ("110m", EmbeddingModel::SnowflakeArcticEmbedM),
    ("137m", EmbeddingModel::SnowflakeArcticEmbedMLong),
    ("335m", EmbeddingModel::SnowflakeArcticEmbedL),
];

/// How to load a local ONNX embedding model.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct FastembedConfig {
    /// The model to load. Short aliases (`nomic`, `minilm`, `bge-small`),
    /// Ollama names with or without a tag (`nomic-embed-text:latest`,
    /// `mxbai-embed-large`), Hugging Face ids in any case and fastembed's
    /// own model names all work; empty means `nomic-embed-text-v1.5`. The
    /// tag is ignored except for Ollama's `snowflake-arctic-embed` size
    /// tags, so `snowflake-arctic-embed:22m` loads the xs model.
    pub model: String,
    /// Where to cache downloaded ONNX files, or `None` for fastembed's
    /// default cache.
    pub cache_directory: Option<PathBuf>,
    /// How many ONNX sessions to keep ready. Values below 1 become 1.
    pub pool_size: usize,
    /// How many texts to send through one session call, or `None` for
    /// fastembed's default batch.
    pub batch_size: Option<usize>,
    /// Whether loading a missing model prints download progress.
    pub show_download_progress: bool,
    /// How many tokens of each input the model reads before truncating the
    /// rest, or `None` for fastembed's 512. The model's tokenizer can lower
    /// it further; [`Self::model_maximum_input_tokens`] gives the length the
    /// model is usually run at.
    pub maximum_input_tokens: Option<u32>,
}

impl Default for FastembedConfig {
    fn default() -> Self {
        Self::new()
    }
}

impl FastembedConfig {
    /// Nomic Embed Text v1.5, one session, no download progress.
    pub fn new() -> Self {
        Self {
            model: DEFAULT_MODEL.to_string(),
            cache_directory: None,
            pool_size: 1,
            batch_size: None,
            show_download_progress: false,
            maximum_input_tokens: None,
        }
    }

    /// all-MiniLM-L6-v2, a smaller and faster model than the default.
    pub fn fast() -> Self {
        Self::new().with_model(FAST_MODEL)
    }

    /// Load `model` rather than Nomic Embed Text v1.5.
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    /// Cache ONNX files under `cache_directory`.
    pub fn with_cache_directory(mut self, cache_directory: impl Into<PathBuf>) -> Self {
        self.cache_directory = Some(cache_directory.into());
        self
    }

    /// Keep `pool_size` ONNX sessions ready.
    pub fn with_pool_size(mut self, pool_size: usize) -> Self {
        self.pool_size = pool_size;
        self
    }

    /// Send `batch_size` texts through one session call.
    pub fn with_batch_size(mut self, batch_size: usize) -> Self {
        self.batch_size = Some(batch_size);
        self
    }

    /// Print download progress when a missing model is fetched.
    pub fn with_show_download_progress(mut self, show_download_progress: bool) -> Self {
        self.show_download_progress = show_download_progress;
        self
    }

    /// Read at most `maximum_input_tokens` tokens of each input. Zero is
    /// refused when the provider is built.
    pub fn with_maximum_input_tokens(mut self, maximum_input_tokens: u32) -> Self {
        self.maximum_input_tokens = Some(maximum_input_tokens);
        self
    }

    /// The sequence length [`Self::model`] is usually run at, the length
    /// Ollama gives it: 2,048 tokens for Nomic Embed Text, 256 for the
    /// all-MiniLM models, 8,192 for BGE-M3 and 512 for every other model.
    /// Pass it to [`Self::with_maximum_input_tokens`] to read that much of
    /// each input; the default stays at fastembed's 512 so stored vectors do
    /// not change. Fails when the model name is not one this config can load.
    pub fn model_maximum_input_tokens(&self) -> Result<u32, ProviderError> {
        self.embedding_model().map(|model| sequence_length(&model))
    }

    pub(crate) fn embedding_model(&self) -> Result<EmbeddingModel, ProviderError> {
        parse_model(&self.model)
    }

    pub(crate) fn model_name(&self) -> &str {
        let trimmed = self.model.trim();
        if trimmed.is_empty() {
            DEFAULT_MODEL
        } else {
            trimmed
        }
    }

    pub(crate) fn checked_maximum_input_tokens(&self) -> Result<Option<u32>, ProviderError> {
        match self.maximum_input_tokens {
            Some(0) => Err(ProviderError::config(
                "maximum input tokens must be at least 1",
            )),
            maximum_input_tokens => Ok(maximum_input_tokens),
        }
    }
}

pub(crate) fn parse_model(name: &str) -> Result<EmbeddingModel, ProviderError> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Ok(EmbeddingModel::NomicEmbedTextV15);
    }
    let (id, tag) = match trimmed.split_once(TAG_SEPARATOR) {
        Some((base, tag)) => (base.trim(), Some(tag.trim())),
        None => (trimmed, None),
    };
    sized(id, tag)
        .or_else(|| alias(&id.to_ascii_lowercase()))
        .or_else(|| by_model_code(id))
        .or_else(|| id.parse().ok())
        .ok_or_else(|| ProviderError::config(format!("unknown embedding model {trimmed}")))
}

fn sized(id: &str, tag: Option<&str>) -> Option<EmbeddingModel> {
    let tag = tag?;
    if !id.eq_ignore_ascii_case(SNOWFLAKE_ARCTIC_EMBED) {
        return None;
    }
    SNOWFLAKE_ARCTIC_EMBED_SIZES
        .iter()
        .find(|(size, _)| size.eq_ignore_ascii_case(tag))
        .map(|(_, model)| model.clone())
}

fn alias(key: &str) -> Option<EmbeddingModel> {
    let model = match key {
        "nomic" | "nomic-embed-text" | "nomic-embed-text-v1.5" => EmbeddingModel::NomicEmbedTextV15,
        "nomic-embed-text-v1" => EmbeddingModel::NomicEmbedTextV1,
        "minilm" | "all-minilm" | "all-minilm-l6-v2" => EmbeddingModel::AllMiniLML6V2,
        "all-minilm-l12-v2" => EmbeddingModel::AllMiniLML12V2,
        "bge" | "bge-small" | "bge-small-en" | "bge-small-en-v1.5" => EmbeddingModel::BGESmallENV15,
        "bge-base" | "bge-base-en" | "bge-base-en-v1.5" => EmbeddingModel::BGEBaseENV15,
        "bge-large" | "bge-large-en" | "bge-large-en-v1.5" => EmbeddingModel::BGELargeENV15,
        "bge-m3" => EmbeddingModel::BGEM3,
        "mxbai-embed-large" => EmbeddingModel::MxbaiEmbedLargeV1,
        SNOWFLAKE_ARCTIC_EMBED | "snowflake-arctic-embed-l" => {
            EmbeddingModel::SnowflakeArcticEmbedL
        }
        "snowflake-arctic-embed-m" => EmbeddingModel::SnowflakeArcticEmbedM,
        "snowflake-arctic-embed-s" => EmbeddingModel::SnowflakeArcticEmbedS,
        "snowflake-arctic-embed-xs" => EmbeddingModel::SnowflakeArcticEmbedXS,
        _ => return None,
    };
    Some(model)
}

fn by_model_code(id: &str) -> Option<EmbeddingModel> {
    full_precision(
        TextEmbedding::list_supported_models()
            .into_iter()
            .filter(|info| info.model_code.eq_ignore_ascii_case(id))
            .map(|info| info.model),
    )
}

fn full_precision(candidates: impl IntoIterator<Item = EmbeddingModel>) -> Option<EmbeddingModel> {
    let candidates: Vec<EmbeddingModel> = candidates.into_iter().collect();
    candidates
        .iter()
        .find(|model| TextEmbedding::get_quantization_mode(model) == QuantizationMode::None)
        .or_else(|| candidates.first())
        .cloned()
}

fn sequence_length(model: &EmbeddingModel) -> u32 {
    match model {
        EmbeddingModel::NomicEmbedTextV1
        | EmbeddingModel::NomicEmbedTextV15
        | EmbeddingModel::NomicEmbedTextV15Q => NOMIC_MAXIMUM_INPUT_TOKENS,
        EmbeddingModel::AllMiniLML6V2
        | EmbeddingModel::AllMiniLML6V2Q
        | EmbeddingModel::AllMiniLML12V2
        | EmbeddingModel::AllMiniLML12V2Q => MINILM_MAXIMUM_INPUT_TOKENS,
        EmbeddingModel::BGEM3 => BGE_M3_MAXIMUM_INPUT_TOKENS,
        _ => DEFAULT_MAXIMUM_INPUT_TOKENS,
    }
}

#[cfg(test)]
mod tests {
    use fastembed::EmbeddingModel;

    use super::DEFAULT_MODEL;
    use super::FAST_MODEL;
    use super::FastembedConfig;
    use super::alias;
    use super::by_model_code;
    use super::full_precision;
    use super::parse_model;
    use crate::provider::ProviderError;

    #[test]
    fn the_default_config_names_nomic_embed_text_v15() {
        let config = FastembedConfig::new();
        assert_eq!(config.model, DEFAULT_MODEL);
        assert_eq!(config.pool_size, 1);
        assert!(!config.show_download_progress);
        assert!(config.cache_directory.is_none());
        assert!(config.batch_size.is_none());
        assert!(config.maximum_input_tokens.is_none());
        assert!(matches!(
            config.embedding_model().unwrap(),
            EmbeddingModel::NomicEmbedTextV15
        ));
    }

    #[test]
    fn the_fast_config_names_all_minilm_l6_v2() {
        let config = FastembedConfig::fast();
        assert_eq!(config.model, FAST_MODEL);
        assert!(matches!(
            config.embedding_model().unwrap(),
            EmbeddingModel::AllMiniLML6V2
        ));
    }

    #[test]
    fn short_aliases_resolve_to_known_models() {
        for (name, expected) in [
            ("nomic", EmbeddingModel::NomicEmbedTextV15),
            ("Nomic-Embed-Text", EmbeddingModel::NomicEmbedTextV15),
            ("minilm", EmbeddingModel::AllMiniLML6V2),
            ("bge-small", EmbeddingModel::BGESmallENV15),
            ("bge-base", EmbeddingModel::BGEBaseENV15),
            ("bge-large", EmbeddingModel::BGELargeENV15),
            ("", EmbeddingModel::NomicEmbedTextV15),
        ] {
            assert_eq!(parse_model(name).unwrap(), expected, "{name}");
        }
    }

    #[test]
    fn ollama_tags_and_aliases_resolve() {
        for (name, expected) in [
            ("nomic-embed-text:latest", EmbeddingModel::NomicEmbedTextV15),
            (" nomic-embed-text:v1.5 ", EmbeddingModel::NomicEmbedTextV15),
            (
                "nomic-embed-text-v1:latest",
                EmbeddingModel::NomicEmbedTextV1,
            ),
            ("mxbai-embed-large", EmbeddingModel::MxbaiEmbedLargeV1),
            ("mxbai-embed-large:335m", EmbeddingModel::MxbaiEmbedLargeV1),
            (
                "snowflake-arctic-embed",
                EmbeddingModel::SnowflakeArcticEmbedL,
            ),
            (
                "snowflake-arctic-embed-l",
                EmbeddingModel::SnowflakeArcticEmbedL,
            ),
            (
                "snowflake-arctic-embed-m",
                EmbeddingModel::SnowflakeArcticEmbedM,
            ),
            (
                "snowflake-arctic-embed-s",
                EmbeddingModel::SnowflakeArcticEmbedS,
            ),
            (
                "snowflake-arctic-embed-xs",
                EmbeddingModel::SnowflakeArcticEmbedXS,
            ),
            (
                "snowflake-arctic-embed:22m",
                EmbeddingModel::SnowflakeArcticEmbedXS,
            ),
            ("bge-small-en", EmbeddingModel::BGESmallENV15),
            ("bge-base-en", EmbeddingModel::BGEBaseENV15),
            ("bge-large-en", EmbeddingModel::BGELargeENV15),
            ("bge-large-en:335m", EmbeddingModel::BGELargeENV15),
            ("bge-m3", EmbeddingModel::BGEM3),
            ("bge-m3:567m", EmbeddingModel::BGEM3),
            ("all-minilm:latest", EmbeddingModel::AllMiniLML6V2),
            ("all-minilm-l12-v2", EmbeddingModel::AllMiniLML12V2),
        ] {
            assert_eq!(parse_model(name).unwrap(), expected, "{name}");
        }
    }

    #[test]
    fn ollamas_snowflake_size_tags_resolve_to_their_own_model() {
        for (name, expected) in [
            (
                "snowflake-arctic-embed:22m",
                EmbeddingModel::SnowflakeArcticEmbedXS,
            ),
            (
                "snowflake-arctic-embed:33m",
                EmbeddingModel::SnowflakeArcticEmbedS,
            ),
            (
                "snowflake-arctic-embed:110m",
                EmbeddingModel::SnowflakeArcticEmbedM,
            ),
            (
                "snowflake-arctic-embed:137m",
                EmbeddingModel::SnowflakeArcticEmbedMLong,
            ),
            (
                "snowflake-arctic-embed:335m",
                EmbeddingModel::SnowflakeArcticEmbedL,
            ),
            (
                " Snowflake-Arctic-Embed : 22M ",
                EmbeddingModel::SnowflakeArcticEmbedXS,
            ),
            (
                "snowflake-arctic-embed:latest",
                EmbeddingModel::SnowflakeArcticEmbedL,
            ),
            (
                "snowflake-arctic-embed",
                EmbeddingModel::SnowflakeArcticEmbedL,
            ),
            (
                "snowflake-arctic-embed-m:22m",
                EmbeddingModel::SnowflakeArcticEmbedM,
            ),
        ] {
            assert_eq!(parse_model(name).unwrap(), expected, "{name}");
        }
    }

    #[test]
    fn a_model_code_resolves_whatever_its_case() {
        assert_eq!(
            parse_model("XENOVA/BGE-Small-EN-v1.5").unwrap(),
            EmbeddingModel::BGESmallENV15
        );
        assert_eq!(parse_model("BAAI/BGE-M3").unwrap(), EmbeddingModel::BGEM3);
    }

    #[test]
    fn a_shared_model_code_resolves_to_the_full_precision_model() {
        assert_eq!(
            parse_model("nomic-ai/nomic-embed-text-v1.5").unwrap(),
            EmbeddingModel::NomicEmbedTextV15
        );
        assert_eq!(
            parse_model("mixedbread-ai/mxbai-embed-large-v1").unwrap(),
            EmbeddingModel::MxbaiEmbedLargeV1
        );
        assert_eq!(
            full_precision([
                EmbeddingModel::NomicEmbedTextV15Q,
                EmbeddingModel::NomicEmbedTextV15,
            ]),
            Some(EmbeddingModel::NomicEmbedTextV15)
        );
        assert_eq!(
            full_precision([
                EmbeddingModel::MxbaiEmbedLargeV1Q,
                EmbeddingModel::MxbaiEmbedLargeV1,
            ]),
            Some(EmbeddingModel::MxbaiEmbedLargeV1)
        );
    }

    #[test]
    fn a_model_code_held_only_by_a_quantised_model_still_resolves() {
        assert_eq!(
            parse_model("Xenova/all-MiniLM-L6-v2").unwrap(),
            EmbeddingModel::AllMiniLML6V2Q
        );
        assert_eq!(
            full_precision([EmbeddingModel::AllMiniLML6V2Q]),
            Some(EmbeddingModel::AllMiniLML6V2Q)
        );
        assert_eq!(full_precision([]), None);
    }

    #[test]
    fn a_fastembed_model_name_resolves_whatever_its_case() {
        for (name, expected) in [
            ("NomicEmbedTextV15Q", EmbeddingModel::NomicEmbedTextV15Q),
            ("bgesmallenv15q", EmbeddingModel::BGESmallENV15Q),
            ("AllMiniLML12V2Q:latest", EmbeddingModel::AllMiniLML12V2Q),
        ] {
            assert_eq!(parse_model(name).unwrap(), expected, "{name}");
        }
    }

    #[test]
    fn an_alias_wins_over_a_model_code() {
        for (name, expected) in [
            ("nomic-embed-text", EmbeddingModel::NomicEmbedTextV15),
            ("NOMIC-EMBED-TEXT-V1", EmbeddingModel::NomicEmbedTextV1),
            ("all-minilm-l6-v2", EmbeddingModel::AllMiniLML6V2),
            ("all-minilm-l12-v2", EmbeddingModel::AllMiniLML12V2),
            ("bge-small-en-v1.5", EmbeddingModel::BGESmallENV15),
            ("bge-m3", EmbeddingModel::BGEM3),
            ("mxbai-embed-large", EmbeddingModel::MxbaiEmbedLargeV1),
            (
                "snowflake-arctic-embed-l",
                EmbeddingModel::SnowflakeArcticEmbedL,
            ),
        ] {
            let key = name.to_ascii_lowercase();
            assert_eq!(alias(&key), Some(expected.clone()), "{name}");
            assert_eq!(parse_model(name).unwrap(), expected, "{name}");
            assert_eq!(by_model_code(name), None, "{name}");
        }
    }

    #[test]
    fn the_model_table_gives_zones_sequence_lengths() {
        for (name, expected) in [
            ("nomic-embed-text-v1", 2048),
            ("nomic-embed-text:latest", 2048),
            ("NomicEmbedTextV15Q", 2048),
            ("all-minilm", 256),
            ("AllMiniLML6V2Q", 256),
            ("all-minilm-l12-v2", 256),
            ("AllMiniLML12V2Q", 256),
            ("bge-m3", 8192),
            ("bge-small-en-v1.5", 512),
            ("mxbai-embed-large", 512),
            ("snowflake-arctic-embed", 512),
            ("", 2048),
        ] {
            let config = FastembedConfig::new().with_model(name);
            assert_eq!(
                config.model_maximum_input_tokens().unwrap(),
                expected,
                "{name}"
            );
        }
        let error = FastembedConfig::new()
            .with_model("not-a-model")
            .model_maximum_input_tokens()
            .unwrap_err();
        assert!(error.to_string().contains("not-a-model"), "{error}");
    }

    #[test]
    fn an_unknown_model_name_is_a_configuration_error() {
        for name in ["not-a-model", "not-a-model:latest", ":latest"] {
            let error = parse_model(name).unwrap_err();
            let rendered = error.to_string();
            assert!(rendered.contains(name), "lost the name: {rendered}");
        }
    }

    #[test]
    fn builders_replace_the_fields_they_name() {
        let config = FastembedConfig::new()
            .with_model("minilm")
            .with_pool_size(4)
            .with_batch_size(8)
            .with_show_download_progress(true)
            .with_cache_directory("/tmp/embeddings")
            .with_maximum_input_tokens(2048);
        assert_eq!(config.model, "minilm");
        assert_eq!(config.pool_size, 4);
        assert_eq!(config.batch_size, Some(8));
        assert!(config.show_download_progress);
        assert_eq!(
            config.cache_directory.as_deref(),
            Some(std::path::Path::new("/tmp/embeddings"))
        );
        assert_eq!(config.maximum_input_tokens, Some(2048));
    }

    #[test]
    fn the_configured_name_is_trimmed() {
        for (name, expected) in [
            (" nomic-embed-text ", "nomic-embed-text"),
            ("\tall-minilm:latest\n", "all-minilm:latest"),
            ("bge-m3", "bge-m3"),
            ("   ", DEFAULT_MODEL),
            ("", DEFAULT_MODEL),
        ] {
            let config = FastembedConfig::new().with_model(name);
            assert_eq!(config.model_name(), expected, "{name:?}");
        }
    }

    #[test]
    fn a_zero_maximum_input_tokens_is_refused() {
        let error = FastembedConfig::new()
            .with_maximum_input_tokens(0)
            .checked_maximum_input_tokens()
            .unwrap_err();
        assert!(matches!(error, ProviderError::Config { .. }), "{error:?}");
        assert!(
            error.to_string().contains("maximum input tokens"),
            "{error}"
        );
        for (config, expected) in [
            (FastembedConfig::new(), None),
            (FastembedConfig::new().with_maximum_input_tokens(1), Some(1)),
        ] {
            assert_eq!(config.checked_maximum_input_tokens().unwrap(), expected);
        }
    }
}
