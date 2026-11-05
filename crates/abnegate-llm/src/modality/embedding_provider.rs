use async_trait::async_trait;

use crate::provider::ProviderError;

pub(crate) const DEFAULT_MAXIMUM_INPUT_TOKENS: u32 = 512;
const MINIMUM_INPUT_CHARACTERS: usize = 512;
const MAXIMUM_INPUT_CHARACTERS: usize = 3000;

/// A source of embeddings.
#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    /// The provider's name, as errors and logs report it.
    fn name(&self) -> &str;
    /// How many values each embedding is expected to hold. Embeddings are
    /// passed on at whatever width the model returns, so a caller that needs
    /// this width compares it with the length of each one.
    fn dimensions(&self) -> u32;
    /// The model this provider embeds with, or [`Self::name`] when it does
    /// not say.
    fn model(&self) -> &str {
        self.name()
    }
    /// The most tokens one input may hold before the model truncates or
    /// refuses it, 512 unless the provider says otherwise.
    fn maximum_input_tokens(&self) -> u32 {
        DEFAULT_MAXIMUM_INPUT_TOKENS
    }
    /// The most characters to send as one input: one and a half for each of
    /// [`Self::maximum_input_tokens`], held between 512 and 3,000, so
    /// identifier-dense code still fits the model's window.
    fn maximum_input_characters(&self) -> usize {
        usize::try_from(self.maximum_input_tokens())
            .unwrap_or(usize::MAX)
            .saturating_mul(3)
            .saturating_div(2)
            .clamp(MINIMUM_INPUT_CHARACTERS, MAXIMUM_INPUT_CHARACTERS)
    }

    /// One embedding for each of `texts`, in the same order.
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, ProviderError>;
    /// The embedding of `text` alone.
    async fn embed_single(&self, text: &str) -> Result<Vec<f32>, ProviderError>;
}
