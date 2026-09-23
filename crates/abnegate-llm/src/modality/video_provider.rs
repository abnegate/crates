use async_trait::async_trait;

use crate::modality::{ModalityError, VideoRequest, VideoResponse};

#[async_trait]
pub trait VideoProvider: Send + Sync {
    fn name(&self) -> &str;

    async fn generate(&self, request: &VideoRequest) -> Result<VideoResponse, ModalityError>;
}
