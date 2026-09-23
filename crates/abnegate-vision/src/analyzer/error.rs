//! What finding a subject can fail with.

use crate::{crop, decode, gravity, preprocess, saliency};

/// Anything [`Analyzer`](crate::Analyzer) can fail with, from decoding the
/// image to rendering its crop.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum AnalyzerError {
    #[error(transparent)]
    Decode(#[from] decode::Error),
    #[error(transparent)]
    Preprocess(#[from] preprocess::Error),
    #[error(transparent)]
    Saliency(#[from] saliency::Error),
    #[error(transparent)]
    Gravity(#[from] gravity::Error),
    #[error(transparent)]
    Crop(#[from] crop::Error),
}
