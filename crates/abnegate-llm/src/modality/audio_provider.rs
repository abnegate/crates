use async_trait::async_trait;

use crate::modality::{AudioResponse, MusicRequest, SfxRequest};
use crate::provider::ProviderError;

#[async_trait]
pub trait AudioProvider: Send + Sync {
    fn name(&self) -> &str;
    fn supported_formats(&self) -> Vec<String>;
    fn max_duration_seconds(&self) -> f64;

    async fn generate_music(&self, request: &MusicRequest) -> Result<AudioResponse, ProviderError>;
    async fn generate_sfx(&self, request: &SfxRequest) -> Result<AudioResponse, ProviderError>;
}
