use std::path::PathBuf;

use fastembed::EmbeddingModel;

use crate::provider::ProviderError;

const DEFAULT_MODEL: &str = "nomic-embed-text-v1.5";
const FAST_MODEL: &str = "all-minilm-l6-v2";

/// How to load a local ONNX embedding model.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct FastembedConfig {
    /// The model to load. Short aliases (`nomic`, `minilm`, `bge-small`)
    /// and Hugging Face ids both work; empty means
    /// `nomic-embed-text-v1.5`.
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

    pub(crate) fn embedding_model(&self) -> Result<EmbeddingModel, ProviderError> {
        parse_model(&self.model)
    }
}

pub(crate) fn parse_model(name: &str) -> Result<EmbeddingModel, ProviderError> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Ok(EmbeddingModel::NomicEmbedTextV15);
    }
    let key = trimmed.to_ascii_lowercase();
    let model = match key.as_str() {
        "nomic" | "nomic-embed-text" | "nomic-embed-text-v1.5" => EmbeddingModel::NomicEmbedTextV15,
        "nomic-embed-text-v1" => EmbeddingModel::NomicEmbedTextV1,
        "minilm" | "all-minilm" | "all-minilm-l6-v2" => EmbeddingModel::AllMiniLML6V2,
        "bge" | "bge-small" | "bge-small-en-v1.5" => EmbeddingModel::BGESmallENV15,
        "bge-base" | "bge-base-en-v1.5" => EmbeddingModel::BGEBaseENV15,
        "bge-large" | "bge-large-en-v1.5" => EmbeddingModel::BGELargeENV15,
        _ => trimmed
            .parse()
            .map_err(|_| ProviderError::config(format!("unknown embedding model {trimmed}")))?,
    };
    Ok(model)
}

#[cfg(test)]
mod tests {
    use super::DEFAULT_MODEL;
    use super::FAST_MODEL;
    use super::FastembedConfig;
    use super::parse_model;
    use fastembed::EmbeddingModel;

    #[test]
    fn the_default_config_names_nomic_embed_text_v15() {
        let config = FastembedConfig::new();
        assert_eq!(config.model, DEFAULT_MODEL);
        assert_eq!(config.pool_size, 1);
        assert!(!config.show_download_progress);
        assert!(config.cache_directory.is_none());
        assert!(config.batch_size.is_none());
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
    fn an_unknown_model_name_is_a_configuration_error() {
        let error = parse_model("not-a-model").unwrap_err();
        let rendered = error.to_string();
        assert!(
            rendered.contains("not-a-model"),
            "lost the name: {rendered}"
        );
    }

    #[test]
    fn builders_replace_the_fields_they_name() {
        let config = FastembedConfig::new()
            .with_model("minilm")
            .with_pool_size(4)
            .with_batch_size(8)
            .with_show_download_progress(true)
            .with_cache_directory("/tmp/embeddings");
        assert_eq!(config.model, "minilm");
        assert_eq!(config.pool_size, 4);
        assert_eq!(config.batch_size, Some(8));
        assert!(config.show_download_progress);
        assert_eq!(
            config.cache_directory.as_deref(),
            Some(std::path::Path::new("/tmp/embeddings"))
        );
    }
}
