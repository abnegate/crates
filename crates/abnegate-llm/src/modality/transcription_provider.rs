use std::path::Path;

use async_trait::async_trait;

use crate::modality::TranscriptionResponse;
use crate::provider::ProviderError;

#[async_trait]
pub trait TranscriptionProvider: Send + Sync {
    fn name(&self) -> &str;

    async fn transcribe(&self, audio_path: &Path) -> Result<TranscriptionResponse, ProviderError>;
}
