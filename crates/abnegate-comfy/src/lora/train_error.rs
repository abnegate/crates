/// What training a LoRA, or preparing its images, can fail with.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum TrainError {
    /// Neither ComfyUI nor a training command is configured.
    #[error("training is not configured")]
    Disabled,
    /// The request cannot be trained on, as the text says.
    #[error("invalid training request: {0}")]
    Invalid(&'static str),
    /// A setting cannot work, as the text says.
    #[error("invalid training configuration: {0}")]
    Configuration(&'static str),
    /// The run, or a step of it, failed.
    #[error("training failed: {0}")]
    Failed(String),
}
