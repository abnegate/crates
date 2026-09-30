use async_trait::async_trait;

use crate::provider::ProviderError;

/// A source of embeddings.
#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    /// The provider's name, as errors and logs report it.
    fn name(&self) -> &str;
    /// How many values each embedding holds.
    fn dimensions(&self) -> u32;

    /// One embedding for each of `texts`, in the same order.
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, ProviderError>;
    /// The embedding of `text` alone.
    async fn embed_single(&self, text: &str) -> Result<Vec<f32>, ProviderError>;
}
