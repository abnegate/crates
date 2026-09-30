use serde::Deserialize;
use serde::Serialize;

use crate::hardware::ModelRecommendation;

/// The best local model for each modality on one machine.
///
/// The default recommends no local model for any modality; each `with_*`
/// method names the one for its modality.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[non_exhaustive]
pub struct RecommendedModels {
    /// Text generation.
    pub llm: ModelRecommendation,
    /// Image generation.
    pub image: ModelRecommendation,
    /// Speech synthesis.
    pub voice: ModelRecommendation,
    /// Music generation.
    pub music: ModelRecommendation,
    /// 3D model generation.
    pub model3d: ModelRecommendation,
    /// Embeddings.
    pub embedding: ModelRecommendation,
    /// Speech to text.
    pub transcription: ModelRecommendation,
}

impl RecommendedModels {
    /// Set the text model.
    pub fn with_llm(mut self, llm: ModelRecommendation) -> Self {
        self.llm = llm;
        self
    }

    /// Set the image generation model.
    pub fn with_image(mut self, image: ModelRecommendation) -> Self {
        self.image = image;
        self
    }

    /// Set the speech synthesis model.
    pub fn with_voice(mut self, voice: ModelRecommendation) -> Self {
        self.voice = voice;
        self
    }

    /// Set the music generation model.
    pub fn with_music(mut self, music: ModelRecommendation) -> Self {
        self.music = music;
        self
    }

    /// Set the 3D model generation model.
    pub fn with_model3d(mut self, model3d: ModelRecommendation) -> Self {
        self.model3d = model3d;
        self
    }

    /// Set the embedding model.
    pub fn with_embedding(mut self, embedding: ModelRecommendation) -> Self {
        self.embedding = embedding;
        self
    }

    /// Set the transcription model.
    pub fn with_transcription(mut self, transcription: ModelRecommendation) -> Self {
        self.transcription = transcription;
        self
    }
}
