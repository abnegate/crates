//! Local ONNX embeddings through fastembed.

mod config;

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use async_trait::async_trait;
use fastembed::TextEmbedding;
use fastembed::TextInitOptions;

pub use crate::modality::vendor::fastembed::config::FastembedConfig;

use crate::modality::EmbeddingProvider;
use crate::provider::ProviderError;

const NAME: &str = "fastembed";

/// Local ONNX embeddings, loaded through fastembed.
///
/// The first call downloads the model into the cache directory when it is
/// not already there. Inference is synchronous inside the process; the
/// [`EmbeddingProvider`] methods run it on `spawn_blocking` so a tokio
/// worker is not stalled, and [`Self::embed_blocking`] is the same work
/// without a runtime.
pub struct FastembedProvider {
    pool: Vec<Arc<Mutex<TextEmbedding>>>,
    next: AtomicUsize,
    dimensions: u32,
    model_name: String,
    batch_size: Option<usize>,
}

impl std::fmt::Debug for FastembedProvider {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FastembedProvider")
            .field("model", &self.model_name)
            .field("dimensions", &self.dimensions)
            .field("pool_size", &self.pool.len())
            .field("batch_size", &self.batch_size)
            .finish()
    }
}

impl FastembedProvider {
    /// Load the model `config` names.
    pub fn new(config: FastembedConfig) -> Result<Self, ProviderError> {
        let model = config.embedding_model()?;
        let dimensions = TextEmbedding::get_model_info(&model)
            .map(|info| info.dim as u32)
            .map_err(|error| ProviderError::config(error.to_string()))?;
        let model_name = model.to_string();
        let pool_size = config.pool_size.max(1);
        let mut pool = Vec::with_capacity(pool_size);
        for _ in 0..pool_size {
            pool.push(Arc::new(Mutex::new(load_session(&config, &model)?)));
        }
        Ok(Self {
            pool,
            next: AtomicUsize::new(0),
            dimensions,
            model_name,
            batch_size: config.batch_size,
        })
    }

    /// The model this provider loaded.
    pub fn model(&self) -> &str {
        &self.model_name
    }

    /// One embedding for each of `texts`, on this thread.
    pub fn embed_blocking(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, ProviderError> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let instance = self.acquire();
        let mut model = instance.lock().map_err(|error| {
            ProviderError::agent(NAME, &format!("model lock poisoned: {error}"))
        })?;
        model
            .embed(texts, self.batch_size)
            .map_err(|error| ProviderError::agent(NAME, &error.to_string()))
    }

    /// The embedding of `text` alone, on this thread.
    pub fn embed_single_blocking(&self, text: &str) -> Result<Vec<f32>, ProviderError> {
        self.embed_blocking(&[text.to_string()])?
            .pop()
            .ok_or_else(|| ProviderError::parse("empty embedding response"))
    }

    fn acquire(&self) -> Arc<Mutex<TextEmbedding>> {
        let index = self.next.fetch_add(1, Ordering::Relaxed) % self.pool.len();
        self.pool[index].clone()
    }
}

fn load_session(
    config: &FastembedConfig,
    model: &fastembed::EmbeddingModel,
) -> Result<TextEmbedding, ProviderError> {
    let mut options = TextInitOptions::new(model.clone())
        .with_show_download_progress(config.show_download_progress);
    if let Some(cache_directory) = &config.cache_directory {
        options = options.with_cache_dir(cache_directory.clone());
    }
    TextEmbedding::try_new(options).map_err(|error| ProviderError::config(error.to_string()))
}

#[async_trait]
impl EmbeddingProvider for FastembedProvider {
    fn name(&self) -> &str {
        NAME
    }

    fn dimensions(&self) -> u32 {
        self.dimensions
    }

    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, ProviderError> {
        let instance = self.acquire();
        let batch_size = self.batch_size;
        let texts = texts.to_vec();
        tokio::task::spawn_blocking(move || {
            let mut model = instance.lock().map_err(|error| {
                ProviderError::agent(NAME, &format!("model lock poisoned: {error}"))
            })?;
            model
                .embed(texts, batch_size)
                .map_err(|error| ProviderError::agent(NAME, &error.to_string()))
        })
        .await
        .map_err(|error| ProviderError::agent(NAME, &format!("embedding task panicked: {error}")))?
    }

    async fn embed_single(&self, text: &str) -> Result<Vec<f32>, ProviderError> {
        self.embed(&[text.to_string()])
            .await?
            .pop()
            .ok_or_else(|| ProviderError::parse("empty embedding response"))
    }
}

#[cfg(test)]
impl FastembedProvider {
    pub(crate) fn for_tests() -> Arc<Self> {
        static CLIENT: std::sync::OnceLock<Arc<FastembedProvider>> = std::sync::OnceLock::new();
        CLIENT
            .get_or_init(|| {
                Arc::new(
                    Self::new(
                        FastembedConfig::fast()
                            .with_pool_size(1)
                            .with_show_download_progress(false),
                    )
                    .expect("test embedding model"),
                )
            })
            .clone()
    }
}

#[cfg(test)]
mod tests {
    use super::FastembedConfig;
    use super::FastembedProvider;
    use crate::modality::EmbeddingProvider;

    #[test]
    fn an_unknown_model_fails_before_the_session_loads() {
        let error =
            FastembedProvider::new(FastembedConfig::new().with_model("not-a-model")).unwrap_err();
        let rendered = error.to_string();
        assert!(
            rendered.contains("not-a-model"),
            "lost the name: {rendered}"
        );
    }

    #[test]
    fn a_local_model_embeds_text_on_this_thread() {
        let provider = FastembedProvider::for_tests();
        let empty = provider.embed_blocking(&[]).unwrap();
        assert!(empty.is_empty());
        let vectors = provider
            .embed_blocking(&["hello".to_string(), "world".to_string()])
            .unwrap();
        assert_eq!(vectors.len(), 2);
        assert_eq!(vectors[0].len(), provider.dimensions() as usize);
        assert_eq!(vectors[1].len(), provider.dimensions() as usize);
        assert_eq!(provider.dimensions(), 384);
        assert_eq!(provider.name(), "fastembed");
    }

    #[tokio::test]
    async fn a_local_model_embeds_text_through_the_trait() {
        let provider = FastembedProvider::for_tests();
        let vectors = provider.embed(&["hello".to_string()]).await.unwrap();
        assert_eq!(vectors[0].len(), provider.dimensions() as usize);
        let single = provider.embed_single("hello").await.unwrap();
        assert_eq!(single.len(), provider.dimensions() as usize);
    }
}
