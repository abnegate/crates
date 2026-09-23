use std::path::Path;

use async_trait::async_trait;

use crate::modality::{ModalityError, TranscriptionResponse};

#[async_trait]
pub trait TranscriptionProvider: Send + Sync {
    fn name(&self) -> &str;

    async fn transcribe(&self, audio_path: &Path) -> Result<TranscriptionResponse, ModalityError>;
}
