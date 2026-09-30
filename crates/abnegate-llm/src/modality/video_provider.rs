use async_trait::async_trait;

use crate::modality::VideoRequest;
use crate::modality::VideoResponse;
use crate::provider::ProviderError;

/// A source of generated video.
#[async_trait]
pub trait VideoProvider: Send + Sync {
    /// The provider's name, as errors and logs report it.
    fn name(&self) -> &str;

    /// Generate the video `request` describes.
    async fn generate(&self, request: &VideoRequest) -> Result<VideoResponse, ProviderError>;
}
