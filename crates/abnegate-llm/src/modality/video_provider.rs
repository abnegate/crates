use async_trait::async_trait;

use crate::modality::{VideoRequest, VideoResponse};
use crate::provider::ProviderError;

#[async_trait]
pub trait VideoProvider: Send + Sync {
    fn name(&self) -> &str;

    async fn generate(&self, request: &VideoRequest) -> Result<VideoResponse, ProviderError>;
}
