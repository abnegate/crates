use serde::{Deserialize, Serialize};

/// One local model, and what running it takes.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ModelRecommendation {
    pub model_name: String,
    pub quantization: Option<String>,
    pub vram_needed_gb: f64,
    pub quality_score: f64,
    pub estimated_speed: String,
    pub can_run_with_others: Vec<String>,
    pub notes: String,
}

impl ModelRecommendation {
    /// `model_name`, needing `vram_needed_gb` of VRAM and scoring
    /// `quality_score` from 0 to 1, with nothing else said about it.
    pub fn new(model_name: impl Into<String>, vram_needed_gb: f64, quality_score: f64) -> Self {
        Self {
            model_name: model_name.into(),
            quantization: None,
            vram_needed_gb,
            quality_score,
            estimated_speed: String::new(),
            can_run_with_others: Vec::new(),
            notes: String::new(),
        }
    }

    /// Set the quantization the model runs at, such as `Q4_K_M`.
    pub fn with_quantization(mut self, quantization: impl Into<String>) -> Self {
        self.quantization = Some(quantization.into());
        self
    }

    /// Set how fast the model runs, in words, such as `~8 tok/s`.
    pub fn with_estimated_speed(mut self, estimated_speed: impl Into<String>) -> Self {
        self.estimated_speed = estimated_speed.into();
        self
    }

    /// Set the modalities whose models fit alongside this one.
    pub fn with_can_run_with_others(mut self, can_run_with_others: Vec<String>) -> Self {
        self.can_run_with_others = can_run_with_others;
        self
    }

    /// Set [`Self::notes`].
    pub fn with_notes(mut self, notes: impl Into<String>) -> Self {
        self.notes = notes.into();
        self
    }
}
