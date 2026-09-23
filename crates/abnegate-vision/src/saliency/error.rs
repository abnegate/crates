//! What running the saliency model can fail with.

use crate::saliency::{INPUT_HEIGHT, INPUT_WIDTH};

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    #[error("model not found at {0}")]
    Missing(String),
    #[error("load saliency model: {0}")]
    Load(String),
    #[error("invalid input tensor length: got {0}")]
    InputLength(usize),
    #[error("run saliency model: {0}")]
    Run(String),
    #[error("saliency model returned no output")]
    MissingOutput,
    #[error("saliency model returned a map shaped {0:?}, not {INPUT_HEIGHT}x{INPUT_WIDTH}")]
    OutputShape(Vec<i64>),
}
