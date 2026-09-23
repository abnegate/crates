#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum TrainError {
    #[error("training is not configured")]
    Disabled,
    #[error("invalid training request: {0}")]
    Invalid(&'static str),
    #[error("invalid training configuration: {0}")]
    Configuration(&'static str),
    #[error("training failed: {0}")]
    Failed(String),
}
