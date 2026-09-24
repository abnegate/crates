use async_trait::async_trait;

use crate::modality::{AudioResponse, Voice, VoiceRequest};
use crate::provider::ProviderError;

#[async_trait]
pub trait VoiceProvider: Send + Sync {
    fn name(&self) -> &str;

    async fn synthesize(&self, request: &VoiceRequest) -> Result<AudioResponse, ProviderError>;
    async fn clone_voice(&self, samples: &[String], name: &str) -> Result<String, ProviderError>;
    async fn list_voices(&self) -> Result<Vec<Voice>, ProviderError>;
}
