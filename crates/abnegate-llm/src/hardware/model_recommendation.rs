use serde::{Deserialize, Serialize};

/// One local model, and what running it takes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelRecommendation {
    pub model_name: String,
    pub quantization: Option<String>,
    pub vram_needed_gb: f64,
    pub quality_score: f64,
    pub estimated_speed: String,
    pub can_run_with_others: Vec<String>,
    pub notes: String,
}
