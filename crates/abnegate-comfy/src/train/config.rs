use crate::lora::TrainError;
use crate::train::PACKAGED_TRAIN_CONFIG;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct TrainConfig {
    pub(crate) passes_per_image: u32,
    pub(crate) min_steps: u32,
    pub(crate) max_steps: u32,
    pub(crate) rank: u32,
    pub(crate) learning_rate: f64,
    pub(crate) lora_dtype: String,
    pub(crate) training_dtype: String,
    pub(crate) resolution: u32,
    pub(crate) bypass_mode: bool,
    pub(crate) gradient_checkpointing: bool,
    pub(crate) checkpoint_depth: u32,
    pub(crate) seed: u64,
}

impl TrainConfig {
    /// Side of the square every training image is read back at.
    pub fn resolution(&self) -> u32 {
        self.resolution
    }

    /// Steps for a dataset of this size, clamped to the configured bounds.
    pub fn steps(&self, image_count: usize) -> u32 {
        u32::try_from(image_count.max(1))
            .unwrap_or(u32::MAX)
            .saturating_mul(self.passes_per_image)
            .clamp(self.min_steps, self.max_steps)
    }
}

pub fn packaged_config() -> Result<TrainConfig, TrainError> {
    serde_json::from_str(PACKAGED_TRAIN_CONFIG)
        .map_err(|error| TrainError::Failed(format!("train config: {error}")))
}
