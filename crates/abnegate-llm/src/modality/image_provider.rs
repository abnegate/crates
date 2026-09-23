use async_trait::async_trait;

use crate::modality::{ImageEditRequest, ImageRequest, ImageResponse, ModalityError};

#[async_trait]
pub trait ImageProvider: Send + Sync {
    fn name(&self) -> &str;
    fn supported_styles(&self) -> Vec<String>;
    fn max_resolution(&self) -> (u32, u32);

    async fn generate(&self, request: &ImageRequest) -> Result<ImageResponse, ModalityError>;
    async fn edit(&self, request: &ImageEditRequest) -> Result<ImageResponse, ModalityError>;
    async fn variations(
        &self,
        image: &[u8],
        count: u32,
    ) -> Result<Vec<ImageResponse>, ModalityError>;
}
