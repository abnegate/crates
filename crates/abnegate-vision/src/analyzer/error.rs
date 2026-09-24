//! What finding a subject can fail with.

use crate::crop::CropError;
use crate::decode::DecodeError;
use crate::gravity::GravityError;
use crate::preprocess::PreprocessError;
use crate::saliency::SaliencyError;

/// Anything [`Analyzer`](crate::Analyzer) can fail with, from decoding the
/// image to rendering its crop.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum AnalyzerError {
    #[error(transparent)]
    Decode(#[from] DecodeError),
    #[error(transparent)]
    Preprocess(#[from] PreprocessError),
    #[error(transparent)]
    Saliency(#[from] SaliencyError),
    #[error(transparent)]
    Gravity(#[from] GravityError),
    #[error(transparent)]
    Crop(#[from] CropError),
}
