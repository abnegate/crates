use async_trait::async_trait;

use crate::modality::{ImageEditRequest, ImageRequest, ImageResponse};
use crate::provider::ProviderError;

#[async_trait]
pub trait ImageProvider: Send + Sync {
    fn name(&self) -> &str;
    fn supported_styles(&self) -> Vec<String>;
    fn max_resolution(&self) -> (u32, u32);

    async fn generate(&self, request: &ImageRequest) -> Result<ImageResponse, ProviderError>;
    async fn edit(&self, request: &ImageEditRequest) -> Result<ImageResponse, ProviderError>;
    async fn variations(
        &self,
        image: &[u8],
        count: u32,
    ) -> Result<Vec<ImageResponse>, ProviderError>;
}
