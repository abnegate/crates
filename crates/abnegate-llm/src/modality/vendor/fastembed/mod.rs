//! Local ONNX embeddings through fastembed.

mod cache;
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
/// [`Self::new`] downloads the model into the cache directory when it is
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
    ///
    /// Loaders of the same model into the same cache directory, in this
    /// process or another, take turns through a lock file under the cache's
    /// `.locks` directory instead of failing each other's download. The lock
    /// is held while the model downloads and while the first ONNX session is
    /// built, so loaders with a warm cache also build their first session
    /// one at a time. Waiting has no timeout and blocks the calling thread;
    /// async callers should run this on `spawn_blocking`. A cache directory
    /// that cannot hold a lock file (read-only, or without file locking)
    /// loads without one.
    pub fn new(config: FastembedConfig) -> Result<Self, ProviderError> {
        let model = config.embedding_model()?;
        let info = TextEmbedding::get_model_info(&model)
            .map_err(|error| ProviderError::config(error.to_string()))?;
        let dimensions = info.dim as u32;
        let model_name = model.to_string();
        let pool_size = config.pool_size.max(1);
        let mut pool = Vec::with_capacity(pool_size);
        pool.push(Arc::new(Mutex::new(load_cached_session(
            &config,
            &model,
            &info.model_code,
        )?)));
        for _ in 1..pool_size {
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

fn load_cached_session(
    config: &FastembedConfig,
    model: &fastembed::EmbeddingModel,
    model_code: &str,
) -> Result<TextEmbedding, ProviderError> {
    while_holding_the_cache(config, model_code, || load_session(config, model))
}

/// `load`'s answer, run once this process holds `model_code`'s lock in the
/// cache `config` resolves to.
fn while_holding_the_cache<T>(
    config: &FastembedConfig,
    model_code: &str,
    load: impl FnOnce() -> Result<T, ProviderError>,
) -> Result<T, ProviderError> {
    let _lock = cache::lock(&cache::directory(config), model_code)?;
    load()
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
    use std::time::Duration;
    use std::time::Instant;

    use fastembed::TextEmbedding;

    use super::FastembedConfig;
    use super::FastembedProvider;
    use super::cache;
    use crate::modality::EmbeddingProvider;

    const HOLD: Duration = Duration::from_secs(1);

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

    #[test]
    fn a_load_starts_only_once_another_loader_releases_the_cache() {
        let directory = tempfile::tempdir().unwrap();
        let config = FastembedConfig::fast().with_cache_directory(directory.path());
        let code = "Qdrant/all-MiniLM-L6-v2-onnx";
        let held = cache::lock(&cache::directory(&config), code)
            .unwrap()
            .expect("the test cache holds a lock");
        let loader = std::thread::spawn(move || {
            super::while_holding_the_cache(&config, code, || Ok(Instant::now()))
        });
        std::thread::sleep(HOLD);
        let released = Instant::now();
        drop(held);
        let started = loader.join().unwrap().unwrap();
        assert!(
            started >= released,
            "started loading {:?} before the cache was released",
            released - started
        );
    }

    #[test]
    fn a_loader_waits_while_another_holds_the_model_cache() {
        let config = FastembedConfig::fast().with_show_download_progress(false);
        let model = config.embedding_model().unwrap();
        let code = TextEmbedding::get_model_info(&model)
            .unwrap()
            .model_code
            .clone();
        let held = cache::lock(&cache::directory(&config), &code)
            .unwrap()
            .expect("the test cache holds a lock");
        let loader = std::thread::spawn(move || {
            let provider = FastembedProvider::new(config);
            (provider, Instant::now())
        });
        std::thread::sleep(HOLD);
        let released = Instant::now();
        drop(held);
        let (provider, loaded) = loader.join().unwrap();
        let provider = provider.unwrap();
        assert!(
            loaded >= released,
            "loaded {:?} before another loader released the cache",
            released - loaded
        );
        assert_eq!(provider.dimensions(), 384);
    }
}
