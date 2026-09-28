use async_trait::async_trait;

use crate::modality::Model3DRequest;
use crate::modality::Model3DResponse;
use crate::provider::ProviderError;

#[async_trait]
pub trait Model3DProvider: Send + Sync {
    fn name(&self) -> &str;

    async fn generate(&self, request: &Model3DRequest) -> Result<Model3DResponse, ProviderError>;
}
