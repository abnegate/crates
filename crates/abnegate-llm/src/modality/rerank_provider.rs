use async_trait::async_trait;

use crate::provider::ProviderError;

/// A cross-encoder that scores how well each document answers a query,
/// reading the query and the document together.
///
/// A higher score is a better match. The scale is the model's own: a
/// caller blending these scores with another signal normalises them first.
/// A failure is passed on as it happened; whether to fall back to a fixed
/// score is the caller's policy.
#[async_trait]
pub trait RerankProvider: Send + Sync {
    /// The provider's name, as errors and logs report it.
    fn name(&self) -> &str;
    /// The model this provider scores with, or [`Self::name`] when it does
    /// not say.
    fn model(&self) -> &str {
        self.name()
    }

    /// One score for each of `documents` against `query`, in the same order.
    /// An empty batch scores nothing and asks nothing of the model.
    async fn rerank(&self, query: &str, documents: &[String]) -> Result<Vec<f32>, ProviderError>;
    /// The score of `document` alone against `query`.
    async fn score(&self, query: &str, document: &str) -> Result<f32, ProviderError>;
}
