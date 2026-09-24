use serde::{Deserialize, Serialize};

use crate::hardware::ModelRecommendation;

/// The best local model for each modality on one machine.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct RecommendedModels {
    pub llm: ModelRecommendation,
    pub image: ModelRecommendation,
    pub voice: ModelRecommendation,
    pub music: ModelRecommendation,
    pub model3d: ModelRecommendation,
    pub embedding: ModelRecommendation,
    pub transcription: ModelRecommendation,
}

impl RecommendedModels {
    /// One recommendation for each modality.
    pub fn new(
        llm: ModelRecommendation,
        image: ModelRecommendation,
        voice: ModelRecommendation,
        music: ModelRecommendation,
        model3d: ModelRecommendation,
        embedding: ModelRecommendation,
        transcription: ModelRecommendation,
    ) -> Self {
        Self {
            llm,
            image,
            voice,
            music,
            model3d,
            embedding,
            transcription,
        }
    }
}
