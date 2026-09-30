use std::path::Path;

use async_trait::async_trait;

use crate::modality::TranscriptionResponse;
use crate::provider::ProviderError;

/// A source of speech-to-text transcriptions.
#[async_trait]
pub trait TranscriptionProvider: Send + Sync {
    /// The provider's name, as errors and logs report it.
    fn name(&self) -> &str;

    /// Transcribe the audio file at `audio_path`.
    async fn transcribe(&self, audio_path: &Path) -> Result<TranscriptionResponse, ProviderError>;
}
