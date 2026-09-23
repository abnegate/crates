use async_trait::async_trait;

use crate::modality::ModalityError;

#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    fn name(&self) -> &str;
    fn dimensions(&self) -> u32;

    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, ModalityError>;
    async fn embed_single(&self, text: &str) -> Result<Vec<f32>, ModalityError>;
}
