//! Local ONNX embeddings through fastembed.

mod cache;
mod config;

use std::sync::Arc;
use std::sync::Mutex;
#[cfg(test)]
use std::sync::OnceLock;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use async_trait::async_trait;
use fastembed::TextEmbedding;
use fastembed::TextInitOptions;

pub use crate::modality::vendor::fastembed::config::FastembedConfig;

use crate::modality::EmbeddingProvider;
use crate::modality::embedding_provider::DEFAULT_MAXIMUM_INPUT_TOKENS;
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
    configured_model: String,
    maximum_input_tokens: u32,
    batch_size: Option<usize>,
}

impl std::fmt::Debug for FastembedProvider {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FastembedProvider")
            .field("model", &self.model_name)
            .field("configured_model", &self.configured_model)
            .field("dimensions", &self.dimensions)
            .field("maximum_input_tokens", &self.maximum_input_tokens)
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
    ///
    /// The provider reports [`FastembedConfig::maximum_input_tokens`], or
    /// fastembed's 512 when unset, lowered to the limit the model's own
    /// tokenizer applies, such as 256 for all-MiniLM-L6-v2.
    pub fn new(config: FastembedConfig) -> Result<Self, ProviderError> {
        let configured_input_tokens = config
            .checked_maximum_input_tokens()?
            .unwrap_or(DEFAULT_MAXIMUM_INPUT_TOKENS);
        let model = config.embedding_model()?;
        let info = TextEmbedding::get_model_info(&model)
            .map_err(|error| ProviderError::config(error.to_string()))?;
        let dimensions = info.dim as u32;
        let model_name = model.to_string();
        let pool_size = config.pool_size.max(1);
        let mut pool = Vec::with_capacity(pool_size);
        let first = load_cached_session(&config, &model, &info.model_code)?;
        let maximum_input_tokens = truncation_length(&first)
            .map_or(configured_input_tokens, |length| {
                configured_input_tokens.min(length)
            });
        pool.push(Arc::new(Mutex::new(first)));
        for _ in 1..pool_size {
            pool.push(Arc::new(Mutex::new(load_session(&config, &model)?)));
        }
        Ok(Self {
            pool,
            next: AtomicUsize::new(0),
            dimensions,
            model_name,
            configured_model: config.model_name().to_string(),
            maximum_input_tokens,
            batch_size: config.batch_size,
        })
    }

    /// The fastembed model this provider loaded, such as `AllMiniLML6V2`.
    /// Deprecated because it shadows [`EmbeddingProvider::model`], which
    /// names the configured model; use [`Self::loaded_model`] instead.
    #[deprecated(
        since = "0.1.10",
        note = "use `loaded_model`; `EmbeddingProvider::model` names the configured model"
    )]
    pub fn model(&self) -> &str {
        self.loaded_model()
    }

    /// The fastembed model this provider loaded, such as `AllMiniLML6V2`.
    /// [`EmbeddingProvider::model`] answers with the name the config gave
    /// instead, such as `all-minilm:latest`.
    pub fn loaded_model(&self) -> &str {
        &self.model_name
    }

    /// One embedding for each of `texts`, on this thread.
    pub fn embed_blocking(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, ProviderError> {
        embed_in_length_order(&self.acquire(), texts, self.batch_size)
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

/// One embedding for each of `texts`, in input order, from a session that
/// saw them shortest first.
fn embed_in_length_order(
    session: &Mutex<TextEmbedding>,
    texts: &[String],
    batch_size: Option<usize>,
) -> Result<Vec<Vec<f32>>, ProviderError> {
    if texts.is_empty() {
        return Ok(Vec::new());
    }
    let order = length_order(texts);
    let sorted: Vec<&str> = order.iter().map(|&index| texts[index].as_str()).collect();
    let embeddings = {
        let mut model = session.lock().map_err(|error| {
            ProviderError::agent(NAME, &format!("model lock poisoned: {error}"))
        })?;
        model
            .embed(&sorted, batch_size)
            .map_err(|error| ProviderError::agent(NAME, &error.to_string()))?
    };
    restore_order(&order, embeddings)
}

/// The indices of `texts`, shortest text first, with texts of equal length
/// kept in input order.
fn length_order(texts: &[String]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..texts.len()).collect();
    order.sort_by_key(|&index| texts[index].len());
    order
}

/// `sorted` put back in input order, where `sorted[position]` belongs at
/// `order[position]`. Fails when the two lengths differ, rather than drop
/// the unmatched items.
fn restore_order<T>(order: &[usize], sorted: Vec<T>) -> Result<Vec<T>, ProviderError> {
    if sorted.len() != order.len() {
        return Err(ProviderError::agent(
            NAME,
            &format!(
                "the model returned {} embeddings for {} texts",
                sorted.len(),
                order.len()
            ),
        ));
    }
    let mut placed: Vec<(usize, T)> = order.iter().copied().zip(sorted).collect();
    placed.sort_unstable_by_key(|&(index, _)| index);
    Ok(placed.into_iter().map(|(_, item)| item).collect())
}

/// The most tokens `session`'s tokenizer keeps of an input, or `None` when
/// it does not truncate.
fn truncation_length(session: &TextEmbedding) -> Option<u32> {
    session
        .tokenizer
        .get_truncation()
        .map(|truncation| u32::try_from(truncation.max_length).unwrap_or(u32::MAX))
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
    if let Some(maximum_input_tokens) = config.maximum_input_tokens {
        options =
            options.with_max_length(usize::try_from(maximum_input_tokens).unwrap_or(usize::MAX));
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

    fn model(&self) -> &str {
        &self.configured_model
    }

    fn maximum_input_tokens(&self) -> u32 {
        self.maximum_input_tokens
    }

    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, ProviderError> {
        let instance = self.acquire();
        let batch_size = self.batch_size;
        let texts = texts.to_vec();
        tokio::task::spawn_blocking(move || embed_in_length_order(&instance, &texts, batch_size))
            .await
            .map_err(|error| {
                ProviderError::agent(NAME, &format!("embedding task panicked: {error}"))
            })?
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
        static CLIENT: OnceLock<Arc<FastembedProvider>> = OnceLock::new();
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
    use std::sync::OnceLock;
    use std::time::Duration;
    use std::time::Instant;

    use fastembed::TextEmbedding;

    use super::FastembedConfig;
    use super::FastembedProvider;
    use super::cache;
    use super::length_order;
    use super::restore_order;
    use crate::modality::EmbeddingProvider;
    #[cfg(feature = "ollama")]
    use crate::modality::vendor::ollama::OllamaEmbeddingConfig;
    #[cfg(feature = "ollama")]
    use crate::modality::vendor::ollama::OllamaEmbeddingProvider;
    use crate::provider::ProviderError;

    const HOLD: Duration = Duration::from_secs(1);
    const LOADED_MODEL: &str = "AllMiniLML6V2";
    const PADDED_MODEL: &str = " all-minilm:latest ";
    #[cfg(feature = "ollama")]
    const OLLAMA_URL: &str = "http://127.0.0.1:11434";
    const TRUNCATED_MODEL: &str = "all-minilm:latest";
    const TRUNCATED_INPUT_TOKENS: u32 = 8;
    const TOKENIZER_INPUT_TOKENS: u32 = 256;
    const SAME_VECTOR: f32 = 1e-5;
    const NEAR_VECTOR: f32 = 1e-3;
    const SHARED_PREFIX: &str = "the quick brown fox jumps over the lazy dog while the sleepy \
        cat watches from a sunny window ledge every single morning";

    fn truncated() -> &'static FastembedProvider {
        static PROVIDER: OnceLock<FastembedProvider> = OnceLock::new();
        PROVIDER.get_or_init(|| {
            FastembedProvider::new(
                FastembedConfig::fast()
                    .with_model(TRUNCATED_MODEL)
                    .with_maximum_input_tokens(TRUNCATED_INPUT_TOKENS),
            )
            .expect("truncated test embedding model")
        })
    }

    fn distance(left: &[f32], right: &[f32]) -> f32 {
        left.iter()
            .zip(right)
            .map(|(left, right)| (left - right).abs())
            .fold(0.0, f32::max)
    }

    fn diverging_tails() -> Vec<String> {
        vec![
            format!("{SHARED_PREFIX} before the rain arrives over the hills"),
            format!("{SHARED_PREFIX} and quantum chromodynamics explains hadron masses"),
        ]
    }

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
    fn a_local_model_reports_its_tokenizers_length_unless_told_otherwise() {
        assert!(FastembedConfig::fast().maximum_input_tokens.is_none());
        let provider = FastembedProvider::for_tests();
        assert_eq!(provider.maximum_input_tokens(), TOKENIZER_INPUT_TOKENS);
        assert_eq!(
            super::truncation_length(&provider.acquire().lock().unwrap()),
            Some(TOKENIZER_INPUT_TOKENS)
        );
        let vectors = provider.embed_blocking(&diverging_tails()).unwrap();
        assert!(
            distance(&vectors[0], &vectors[1]) > SAME_VECTOR,
            "the default length dropped the tails"
        );
    }

    #[test]
    fn a_local_model_ignores_text_past_its_maximum_input_tokens() {
        let vectors = truncated().embed_blocking(&diverging_tails()).unwrap();
        assert!(
            distance(&vectors[0], &vectors[1]) <= SAME_VECTOR,
            "read past {TRUNCATED_INPUT_TOKENS} tokens"
        );
    }

    #[test]
    fn a_local_model_reports_its_input_limit() {
        let provider = truncated();
        assert_eq!(provider.maximum_input_tokens(), TRUNCATED_INPUT_TOKENS);
        assert_eq!(provider.maximum_input_characters(), 512);
    }

    #[test]
    fn the_loaded_model_names_the_fastembed_variant() {
        assert_eq!(truncated().loaded_model(), LOADED_MODEL);
        assert_eq!(FastembedProvider::for_tests().loaded_model(), LOADED_MODEL);
    }

    #[test]
    #[allow(deprecated)]
    fn the_deprecated_model_still_names_the_loaded_model() {
        let provider = truncated();
        assert_eq!(provider.model(), provider.loaded_model());
        assert_eq!(provider.model(), LOADED_MODEL);
    }

    #[test]
    fn the_trait_model_names_the_configured_model() {
        assert_eq!(EmbeddingProvider::model(truncated()), TRUNCATED_MODEL);
        let default = FastembedProvider::for_tests();
        assert_eq!(EmbeddingProvider::model(&*default), "all-minilm-l6-v2");
        let padded = FastembedProvider::new(FastembedConfig::fast().with_model(PADDED_MODEL))
            .expect("padded test embedding model");
        assert_eq!(EmbeddingProvider::model(&padded), PADDED_MODEL.trim());
        assert_eq!(padded.loaded_model(), LOADED_MODEL);
    }

    #[test]
    fn a_zero_maximum_input_tokens_fails_before_the_model_downloads() {
        let directory = tempfile::tempdir().unwrap();
        let error = FastembedProvider::new(
            FastembedConfig::fast()
                .with_cache_directory(directory.path())
                .with_maximum_input_tokens(0),
        )
        .unwrap_err();
        assert!(matches!(error, ProviderError::Config { .. }), "{error:?}");
        assert!(
            error.to_string().contains("maximum input tokens"),
            "{error}"
        );
        assert!(
            std::fs::read_dir(directory.path())
                .unwrap()
                .next()
                .is_none(),
            "touched the cache before refusing the length"
        );
    }

    #[test]
    fn a_short_embedding_response_is_an_error_not_a_lost_vector() {
        let order = [2, 0, 1];
        let error = restore_order(&order, vec!["c", "a"]).unwrap_err();
        assert!(matches!(error, ProviderError::Agent { .. }), "{error:?}");
        let rendered = error.to_string();
        assert!(rendered.contains("fastembed"), "{rendered}");
        assert!(rendered.contains("2 embeddings for 3 texts"), "{rendered}");
        assert!(restore_order(&order, vec!["c", "a", "b", "d"]).is_err());
        assert!(restore_order::<&str>(&[], vec!["a"]).is_err());
    }

    #[test]
    fn a_batch_is_ordered_by_length_and_restored() {
        let texts: Vec<String> = ["ccc", "a", "bb", "d", "", "eee"]
            .into_iter()
            .map(String::from)
            .collect();
        let order = length_order(&texts);
        assert_eq!(order, [4, 1, 3, 2, 0, 5]);
        let sorted: Vec<String> = order.iter().map(|&index| texts[index].clone()).collect();
        assert_eq!(sorted, ["", "a", "d", "bb", "ccc", "eee"]);
        assert_eq!(restore_order(&order, sorted).unwrap(), texts);
        assert!(length_order(&[]).is_empty());
        assert!(restore_order::<String>(&[], Vec::new()).unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_mixed_batch_comes_back_in_input_order() {
        let provider = FastembedProvider::for_tests();
        let texts = vec![
            format!("{SHARED_PREFIX} and then some more words to make it longest"),
            "hi".to_string(),
            SHARED_PREFIX.to_string(),
        ];
        let singles: Vec<Vec<f32>> = texts
            .iter()
            .map(|text| provider.embed_single_blocking(text).unwrap())
            .collect();
        let blocking = provider.embed_blocking(&texts).unwrap();
        let through_the_trait = provider.embed(&texts).await.unwrap();
        for (position, single) in singles.iter().enumerate() {
            assert!(
                distance(&blocking[position], single) < NEAR_VECTOR,
                "blocking embedding {position} moved"
            );
            assert!(
                distance(&through_the_trait[position], single) < NEAR_VECTOR,
                "trait embedding {position} moved"
            );
        }
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

    #[cfg(feature = "ollama")]
    fn widths(name: &str) -> (u32, u32) {
        let model = FastembedConfig::new()
            .with_model(name)
            .embedding_model()
            .unwrap();
        let local = TextEmbedding::get_model_info(&model).unwrap().dim as u32;
        let ollama = OllamaEmbeddingProvider::new(OllamaEmbeddingConfig::new(OLLAMA_URL, name))
            .unwrap()
            .dimensions();
        (local, ollama)
    }

    #[cfg(feature = "ollama")]
    #[test]
    fn the_local_and_ollama_widths_agree_except_where_known_to_differ() {
        let agreements = [
            ("nomic-embed-text", 768),
            ("nomic-embed-text-v1", 768),
            ("nomic-embed-text-v1.5", 768),
            ("mxbai-embed-large", 1024),
            ("snowflake-arctic-embed", 1024),
            ("snowflake-arctic-embed:latest", 1024),
            ("snowflake-arctic-embed:22m", 384),
            ("snowflake-arctic-embed:33m", 384),
            ("snowflake-arctic-embed:110m", 768),
            ("snowflake-arctic-embed:137m", 768),
            ("snowflake-arctic-embed:335m", 1024),
            ("bge-small-en", 384),
            ("bge-small-en-v1.5", 384),
            ("all-minilm", 384),
            ("all-minilm-l6-v2", 384),
        ];
        for (name, width) in agreements {
            let (local, ollama) = widths(name);
            assert_eq!(ollama, width, "Ollama's width for {name}");
            assert_eq!(local, ollama, "fastembed's width for {name}");
        }
        let disagreements = [
            ("bge-large-en", 1024, 768),
            ("bge-m3", 1024, 768),
            ("snowflake-arctic-embed-s", 384, 768),
            ("all-minilm-l12-v2", 384, 768),
        ];
        for (name, expected_local, expected_ollama) in disagreements {
            let (local, ollama) = widths(name);
            assert_eq!(local, expected_local, "fastembed's width for {name}");
            assert_eq!(ollama, expected_ollama, "Ollama's width for {name}");
            assert_ne!(local, ollama, "{name} no longer disagrees; move it");
        }
    }
}
