//! What running the saliency model can fail with.

use crate::saliency::{INPUT_HEIGHT, INPUT_WIDTH};

/// A failure loading or running the saliency model.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SaliencyError {
    /// No file exists at the model path.
    #[error("model not found at {0}")]
    Missing(String),
    /// ONNX Runtime could not load the model.
    #[error("load saliency model: {0}")]
    Load(String),
    /// The input tensor does not hold [`INPUT_LENGTH`](crate::saliency::INPUT_LENGTH) values.
    #[error("invalid input tensor length: got {0}")]
    InputLength(usize),
    /// ONNX Runtime failed running the model.
    #[error("run saliency model: {0}")]
    Run(String),
    /// The model produced no saliency map.
    #[error("saliency model returned no output")]
    MissingOutput,
    /// The model produced a map of some other shape.
    #[error("saliency model returned a map shaped {0:?}, not {INPUT_HEIGHT}x{INPUT_WIDTH}")]
    OutputShape(Vec<i64>),
}
