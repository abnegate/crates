use async_trait::async_trait;

use crate::modality::ImageEditRequest;
use crate::modality::ImageRequest;
use crate::modality::ImageResponse;
use crate::provider::ProviderError;

/// A source of generated images.
///
/// An operation a provider does not offer fails with
/// [`ProviderError::unsupported`].
#[async_trait]
pub trait ImageProvider: Send + Sync {
    /// The provider's name, as errors and logs report it.
    fn name(&self) -> &str;
    /// The styles an [`ImageRequest::style`] may name.
    fn supported_styles(&self) -> Vec<String>;
    /// The largest width and height, in pixels, this provider generates.
    fn maximum_resolution(&self) -> (u32, u32);

    /// Generate the image `request` describes.
    async fn generate(&self, request: &ImageRequest) -> Result<ImageResponse, ProviderError>;
    /// Change an existing image as `request` says.
    async fn edit(&self, request: &ImageEditRequest) -> Result<ImageResponse, ProviderError>;
    /// Make `count` variations of the encoded `image`.
    async fn variations(
        &self,
        image: &[u8],
        count: u32,
    ) -> Result<Vec<ImageResponse>, ProviderError>;
}
