use std::time::Duration;

use async_trait::async_trait;

use crate::modality::{AudioResponse, MusicRequest, SoundEffectRequest};
use crate::provider::ProviderError;

/// A provider that generates music and sound effects.
#[async_trait]
pub trait AudioProvider: Send + Sync {
    /// The provider's name, for logs and error messages.
    fn name(&self) -> &str;
    /// The audio formats the provider encodes, such as `wav` or `mp3`.
    fn supported_formats(&self) -> Vec<String>;
    /// The longest clip this provider generates.
    fn maximum_duration(&self) -> Duration;

    /// Music as `request` describes it.
    async fn generate_music(&self, request: &MusicRequest) -> Result<AudioResponse, ProviderError>;
    /// A sound effect as `request` describes it.
    async fn generate_sound_effect(
        &self,
        request: &SoundEffectRequest,
    ) -> Result<AudioResponse, ProviderError>;
}
