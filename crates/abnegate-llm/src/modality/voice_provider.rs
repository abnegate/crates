use async_trait::async_trait;

use crate::modality::AudioResponse;
use crate::modality::Voice;
use crate::modality::VoiceRequest;
use crate::provider::ProviderError;

/// A source of synthesized speech.
#[async_trait]
pub trait VoiceProvider: Send + Sync {
    /// The provider's name, as errors and logs report it.
    fn name(&self) -> &str;

    /// Speak the text `request` holds.
    async fn synthesize(&self, request: &VoiceRequest) -> Result<AudioResponse, ProviderError>;
    /// Make a voice called `name` from the recordings at `samples`,
    /// returning the id a [`VoiceRequest::voice_id`] names it by.
    async fn clone_voice(&self, samples: &[String], name: &str) -> Result<String, ProviderError>;
    /// Every voice the provider offers.
    async fn list_voices(&self) -> Result<Vec<Voice>, ProviderError>;
}
