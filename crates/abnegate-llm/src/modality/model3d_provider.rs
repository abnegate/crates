use async_trait::async_trait;

use crate::modality::Model3DRequest;
use crate::modality::Model3DResponse;
use crate::provider::ProviderError;

/// A source of generated 3D models.
#[async_trait]
pub trait Model3DProvider: Send + Sync {
    /// The provider's name, as errors and logs report it.
    fn name(&self) -> &str;

    /// Generate the model `request` describes.
    async fn generate(&self, request: &Model3DRequest) -> Result<Model3DResponse, ProviderError>;
}
