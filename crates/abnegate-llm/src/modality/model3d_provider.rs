use async_trait::async_trait;

use crate::modality::{ModalityError, Model3DRequest, Model3DResponse};

#[async_trait]
pub trait Model3DProvider: Send + Sync {
    fn name(&self) -> &str;

    async fn generate(&self, request: &Model3DRequest) -> Result<Model3DResponse, ModalityError>;
}
