//! One trait per modality, their request and response types, the
//! configuration that names a provider for each, and the vendor-native
//! clients.

pub mod config;
pub mod vendor;

mod ai_error;
mod audio_provider;
mod audio_response;
mod client;
mod embedding_provider;
mod error;
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
mod sfx_request;
mod text_provider;
mod text_request;
mod text_response;
mod transcription_provider;
mod transcription_response;
mod transcription_segment;
mod video_provider;
mod video_request;
mod video_response;
mod voice_info;
mod voice_provider;
mod voice_request;

pub use crate::modality::ai_error::AiError;
pub use crate::modality::audio_provider::AudioProvider;
pub use crate::modality::audio_response::AudioResponse;
pub use crate::modality::client::{AiClient, Exchange};
pub use crate::modality::config::{
    AudioProviderConfig, EmbeddingProviderConfig, ImageProviderConfig, Model3DProviderConfig,
    ProviderConfig, TextProviderConfig, TranscriptionProviderConfig, VideoProviderConfig,
    VoiceProviderConfig,
};
pub use crate::modality::embedding_provider::EmbeddingProvider;
pub use crate::modality::error::ModalityError;
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
pub use crate::modality::sfx_request::SfxRequest;
pub use crate::modality::text_provider::TextProvider;
pub use crate::modality::text_request::TextRequest;
pub use crate::modality::text_response::TextResponse;
pub use crate::modality::transcription_provider::TranscriptionProvider;
pub use crate::modality::transcription_response::TranscriptionResponse;
pub use crate::modality::transcription_segment::TranscriptionSegment;
pub use crate::modality::video_provider::VideoProvider;
pub use crate::modality::video_request::VideoRequest;
pub use crate::modality::video_response::VideoResponse;
pub use crate::modality::voice_info::VoiceInfo;
pub use crate::modality::voice_provider::VoiceProvider;
pub use crate::modality::voice_request::VoiceRequest;

#[cfg(test)]
mod tests {
    use std::path::Path;

    use async_trait::async_trait;
    use futures::Stream;

    use super::*;

    struct Everything;

    #[async_trait]
    impl TextProvider for Everything {
        fn name(&self) -> &str {
            "everything"
        }

        fn supports_structured_output(&self) -> bool {
            false
        }

        fn max_context_tokens(&self) -> u32 {
            0
        }

        async fn complete(&self, _request: &TextRequest) -> Result<TextResponse, ModalityError> {
            Err(ModalityError::Unsupported("complete".into()))
        }

        async fn complete_structured(
            &self,
            _request: &TextRequest,
        ) -> Result<serde_json::Value, ModalityError> {
            Err(ModalityError::Unsupported("complete_structured".into()))
        }

        async fn stream_complete(
            &self,
            _request: &TextRequest,
        ) -> Result<
            Box<dyn Stream<Item = Result<String, ModalityError>> + Send + Unpin>,
            ModalityError,
        > {
            Err(ModalityError::Unsupported("stream_complete".into()))
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

        fn max_resolution(&self) -> (u32, u32) {
            (0, 0)
        }

        async fn generate(&self, _request: &ImageRequest) -> Result<ImageResponse, ModalityError> {
            Err(ModalityError::Unsupported("generate".into()))
        }

        async fn edit(&self, _request: &ImageEditRequest) -> Result<ImageResponse, ModalityError> {
            Err(ModalityError::Unsupported("edit".into()))
        }

        async fn variations(
            &self,
            _image: &[u8],
            _count: u32,
        ) -> Result<Vec<ImageResponse>, ModalityError> {
            Err(ModalityError::Unsupported("variations".into()))
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

        fn max_duration_seconds(&self) -> f64 {
            0.0
        }

        async fn generate_music(
            &self,
            _request: &MusicRequest,
        ) -> Result<AudioResponse, ModalityError> {
            Err(ModalityError::Unsupported("generate_music".into()))
        }

        async fn generate_sfx(
            &self,
            _request: &SfxRequest,
        ) -> Result<AudioResponse, ModalityError> {
            Err(ModalityError::Unsupported("generate_sfx".into()))
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
        ) -> Result<AudioResponse, ModalityError> {
            Err(ModalityError::Unsupported("synthesize".into()))
        }

        async fn clone_voice(
            &self,
            _samples: &[String],
            _name: &str,
        ) -> Result<String, ModalityError> {
            Err(ModalityError::Unsupported("clone_voice".into()))
        }

        async fn list_voices(&self) -> Result<Vec<VoiceInfo>, ModalityError> {
            Err(ModalityError::Unsupported("list_voices".into()))
        }
    }

    #[async_trait]
    impl VideoProvider for Everything {
        fn name(&self) -> &str {
            "everything"
        }

        async fn generate(&self, _request: &VideoRequest) -> Result<VideoResponse, ModalityError> {
            Err(ModalityError::Unsupported("generate".into()))
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
        ) -> Result<Model3DResponse, ModalityError> {
            Err(ModalityError::Unsupported("generate".into()))
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

        async fn embed(&self, _texts: &[String]) -> Result<Vec<Vec<f32>>, ModalityError> {
            Err(ModalityError::Unsupported("embed".into()))
        }

        async fn embed_single(&self, _text: &str) -> Result<Vec<f32>, ModalityError> {
            Err(ModalityError::Unsupported("embed_single".into()))
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
        ) -> Result<TranscriptionResponse, ModalityError> {
            Err(ModalityError::Unsupported("transcribe".into()))
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
        use crate::modality::vendor::OpenAIProvider;

        let provider = OpenAIProvider::new("key");
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
