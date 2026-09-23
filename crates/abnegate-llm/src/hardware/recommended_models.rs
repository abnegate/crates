use serde::{Deserialize, Serialize};

use crate::hardware::ModelRecommendation;

/// The best local model for each modality on one machine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecommendedModels {
    pub llm: ModelRecommendation,
    pub image: ModelRecommendation,
    pub voice: ModelRecommendation,
    pub music: ModelRecommendation,
    pub model3d: ModelRecommendation,
    pub embedding: ModelRecommendation,
    pub transcription: ModelRecommendation,
}
