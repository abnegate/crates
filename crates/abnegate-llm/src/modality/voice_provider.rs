use async_trait::async_trait;

use crate::modality::{AudioResponse, ModalityError, VoiceInfo, VoiceRequest};

#[async_trait]
pub trait VoiceProvider: Send + Sync {
    fn name(&self) -> &str;

    async fn synthesize(&self, request: &VoiceRequest) -> Result<AudioResponse, ModalityError>;
    async fn clone_voice(&self, samples: &[String], name: &str) -> Result<String, ModalityError>;
    async fn list_voices(&self) -> Result<Vec<VoiceInfo>, ModalityError>;
}
