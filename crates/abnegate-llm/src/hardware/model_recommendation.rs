use serde::{Deserialize, Serialize};

/// The model name a slot with no local model holds, which
/// [`CostEstimator::with_hardware`](crate::CostEstimator::with_hardware)
/// never offers.
pub(crate) const NO_LOCAL_MODEL: &str = "none";

/// One local model, and what running it takes.
///
/// The default is no local model at all, which the cost estimator never
/// offers for the modality.
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

impl Default for ModelRecommendation {
    fn default() -> Self {
        Self::new(NO_LOCAL_MODEL)
    }
}

impl ModelRecommendation {
    /// `model_name`, needing no VRAM and scoring zero, with nothing else said
    /// about it.
    pub fn new(model_name: impl Into<String>) -> Self {
        Self {
            model_name: model_name.into(),
            quantization: None,
            vram_needed_gb: 0.0,
            quality_score: 0.0,
            estimated_speed: String::new(),
            can_run_with_others: Vec::new(),
            notes: String::new(),
        }
    }

    /// Set the VRAM, in gigabytes, the model needs loaded.
    pub fn with_vram_needed_gb(mut self, vram_needed_gb: f64) -> Self {
        self.vram_needed_gb = vram_needed_gb;
        self
    }

    /// Set how good the model's output is, from 0 to 1.
    pub fn with_quality_score(mut self, quality_score: f64) -> Self {
        self.quality_score = quality_score;
        self
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
