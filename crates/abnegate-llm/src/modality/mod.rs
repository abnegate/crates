//! One trait per modality, their request and response types, the
//! configuration that names a provider for each, and the vendor-native
//! clients.
//!
//! [`TextProvider`] is the text modality's contract: a system and a user
//! prompt in, prose or a schema-shaped [`StructuredResponse`] out. It is the
//! contract the vendor-native clients in [`vendor`] satisfy.
//! [`CompletionProvider`](crate::CompletionProvider) is the conversation
//! contract an OpenAI-compatible endpoint, a [`Router`](crate::Router) or a
//! coding agent satisfies, and [`CompletionBridge`] adapts any of those into
//! a `TextProvider`, so an [`AiClient`] can run on a local model server or a
//! fallback chain as readily as on a vendor API.

pub mod config;
pub mod vendor;

mod ai_client;
mod ai_error;
mod audio_provider;
mod audio_response;
mod completion_bridge;
mod embedding_provider;
mod exchange;
mod image_edit_request;
mod image_provider;
mod image_request;
mod image_response;
mod model3d_format;
mod model3d_provider;
mod model3d_request;
mod model3d_response;
mod music_request;
mod response_format;
mod sound_effect_request;
mod structured_response;
mod text_provider;
mod text_request;
mod text_response;
mod transcription_provider;
mod transcription_response;
mod transcription_segment;
mod video_provider;
mod video_request;
mod video_response;
mod voice;
mod voice_provider;
mod voice_request;

pub use crate::modality::ai_client::AiClient;
pub use crate::modality::ai_error::AiError;
pub use crate::modality::audio_provider::AudioProvider;
pub use crate::modality::audio_response::AudioResponse;
pub use crate::modality::completion_bridge::CompletionBridge;
pub use crate::modality::config::{
    AudioProviderConfig, EmbeddingProviderConfig, ImageProviderConfig, Model3DProviderConfig,
    ProviderConfig, TextProviderConfig, TranscriptionProviderConfig, VideoProviderConfig,
    VoiceProviderConfig,
};
pub use crate::modality::embedding_provider::EmbeddingProvider;
pub use crate::modality::exchange::Exchange;
pub use crate::modality::image_edit_request::ImageEditRequest;
pub use crate::modality::image_provider::ImageProvider;
pub use crate::modality::image_request::ImageRequest;
pub use crate::modality::image_response::ImageResponse;
pub use crate::modality::model3d_format::Model3DFormat;
pub use crate::modality::model3d_provider::Model3DProvider;
pub use crate::modality::model3d_request::Model3DRequest;
pub use crate::modality::model3d_response::Model3DResponse;
pub use crate::modality::music_request::MusicRequest;
pub use crate::modality::response_format::ResponseFormat;
pub use crate::modality::sound_effect_request::SoundEffectRequest;
pub use crate::modality::structured_response::StructuredResponse;
pub use crate::modality::text_provider::TextProvider;
pub use crate::modality::text_request::TextRequest;
pub use crate::modality::text_response::TextResponse;
pub use crate::modality::transcription_provider::TranscriptionProvider;
pub use crate::modality::transcription_response::TranscriptionResponse;
pub use crate::modality::transcription_segment::TranscriptionSegment;
pub use crate::modality::video_provider::VideoProvider;
pub use crate::modality::video_request::VideoRequest;
pub use crate::modality::video_response::VideoResponse;
pub use crate::modality::voice::Voice;
pub use crate::modality::voice_provider::VoiceProvider;
pub use crate::modality::voice_request::VoiceRequest;

#[cfg(test)]
mod tests {
    use std::path::Path;

    use async_trait::async_trait;
    use futures::Stream;

    use super::*;
    use crate::provider::ProviderError;

    struct Everything;

    #[async_trait]
    impl TextProvider for Everything {
        fn name(&self) -> &str {
            "everything"
        }

        fn supports_structured_output(&self) -> bool {
            false
        }

        fn maximum_context_tokens(&self) -> u32 {
            0
        }

        async fn complete(&self, _request: &TextRequest) -> Result<TextResponse, ProviderError> {
            Err(ProviderError::unsupported("complete"))
        }

        async fn complete_structured(
            &self,
            _request: &TextRequest,
        ) -> Result<StructuredResponse, ProviderError> {
            Err(ProviderError::unsupported("complete_structured"))
        }

        async fn stream_complete(
            &self,
            _request: &TextRequest,
        ) -> Result<
            Box<dyn Stream<Item = Result<String, ProviderError>> + Send + Unpin>,
            ProviderError,
        > {
            Err(ProviderError::unsupported("stream_complete"))
        }
    }

    #[async_trait]
    impl ImageProvider for Everything {
        fn name(&self) -> &str {
            "everything"
        }

        fn supported_styles(&self) -> Vec<String> {
            Vec::new()
        }

        fn maximum_resolution(&self) -> (u32, u32) {
            (0, 0)
        }

        async fn generate(&self, _request: &ImageRequest) -> Result<ImageResponse, ProviderError> {
            Err(ProviderError::unsupported("generate"))
        }

        async fn edit(&self, _request: &ImageEditRequest) -> Result<ImageResponse, ProviderError> {
            Err(ProviderError::unsupported("edit"))
        }

        async fn variations(
            &self,
            _image: &[u8],
            _count: u32,
        ) -> Result<Vec<ImageResponse>, ProviderError> {
            Err(ProviderError::unsupported("variations"))
        }
    }

    #[async_trait]
    impl AudioProvider for Everything {
        fn name(&self) -> &str {
            "everything"
        }

        fn supported_formats(&self) -> Vec<String> {
            Vec::new()
        }

        fn maximum_duration_seconds(&self) -> f64 {
            0.0
        }

        async fn generate_music(
            &self,
            _request: &MusicRequest,
        ) -> Result<AudioResponse, ProviderError> {
            Err(ProviderError::unsupported("generate_music"))
        }

        async fn generate_sound_effect(
            &self,
            _request: &SoundEffectRequest,
        ) -> Result<AudioResponse, ProviderError> {
            Err(ProviderError::unsupported("generate_sound_effect"))
        }
    }

    #[async_trait]
    impl VoiceProvider for Everything {
        fn name(&self) -> &str {
            "everything"
        }

        async fn synthesize(
            &self,
            _request: &VoiceRequest,
        ) -> Result<AudioResponse, ProviderError> {
            Err(ProviderError::unsupported("synthesize"))
        }

        async fn clone_voice(
            &self,
            _samples: &[String],
            _name: &str,
        ) -> Result<String, ProviderError> {
            Err(ProviderError::unsupported("clone_voice"))
        }

        async fn list_voices(&self) -> Result<Vec<Voice>, ProviderError> {
            Err(ProviderError::unsupported("list_voices"))
        }
    }

    #[async_trait]
    impl VideoProvider for Everything {
        fn name(&self) -> &str {
            "everything"
        }

        async fn generate(&self, _request: &VideoRequest) -> Result<VideoResponse, ProviderError> {
            Err(ProviderError::unsupported("generate"))
        }
    }

    #[async_trait]
    impl Model3DProvider for Everything {
        fn name(&self) -> &str {
            "everything"
        }

        async fn generate(
            &self,
            _request: &Model3DRequest,
        ) -> Result<Model3DResponse, ProviderError> {
            Err(ProviderError::unsupported("generate"))
        }
    }

    #[async_trait]
    impl EmbeddingProvider for Everything {
        fn name(&self) -> &str {
            "everything"
        }

        fn dimensions(&self) -> u32 {
            0
        }

        async fn embed(&self, _texts: &[String]) -> Result<Vec<Vec<f32>>, ProviderError> {
            Err(ProviderError::unsupported("embed"))
        }

        async fn embed_single(&self, _text: &str) -> Result<Vec<f32>, ProviderError> {
            Err(ProviderError::unsupported("embed_single"))
        }
    }

    #[async_trait]
    impl TranscriptionProvider for Everything {
        fn name(&self) -> &str {
            "everything"
        }

        async fn transcribe(
            &self,
            _audio_path: &Path,
        ) -> Result<TranscriptionResponse, ProviderError> {
            Err(ProviderError::unsupported("transcribe"))
        }
    }

    #[test]
    fn every_modality_trait_is_object_safe() {
        let _text: Box<dyn TextProvider> = Box::new(Everything);
        let _image: Box<dyn ImageProvider> = Box::new(Everything);
        let _audio: Box<dyn AudioProvider> = Box::new(Everything);
        let _voice: Box<dyn VoiceProvider> = Box::new(Everything);
        let _video: Box<dyn VideoProvider> = Box::new(Everything);
        let _model3d: Box<dyn Model3DProvider> = Box::new(Everything);
        let _embedding: Box<dyn EmbeddingProvider> = Box::new(Everything);
        let _transcription: Box<dyn TranscriptionProvider> = Box::new(Everything);
    }

    #[cfg(feature = "openai")]
    #[test]
    fn the_openai_client_satisfies_the_traits_it_claims() {
        use crate::modality::vendor::OpenAiProvider;

        let provider = OpenAiProvider::new("key");
        let _text: &dyn TextProvider = &provider;
        let _image: &dyn ImageProvider = &provider;
        let _embedding: &dyn EmbeddingProvider = &provider;
        let _transcription: &dyn TranscriptionProvider = &provider;
        let _boxed: Box<dyn TextProvider> = Box::new(provider);
    }

    #[cfg(feature = "anthropic")]
    #[test]
    fn the_anthropic_client_satisfies_the_text_trait() {
        use crate::modality::vendor::AnthropicProvider;

        let provider = AnthropicProvider::new("key");
        let _text: &dyn TextProvider = &provider;
        let _boxed: Box<dyn TextProvider> = Box::new(provider);
    }

    #[cfg(feature = "google")]
    #[test]
    fn the_gemini_client_satisfies_the_text_trait() {
        use crate::modality::vendor::GeminiProvider;

        let provider = GeminiProvider::new("key");
        let _text: &dyn TextProvider = &provider;
        let _boxed: Box<dyn TextProvider> = Box::new(provider);
    }
}
