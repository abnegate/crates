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
    /// The bytes could not be decoded as an image.
    #[error(transparent)]
    Decode(#[from] DecodeError),
    /// The decoded image could not be fitted into the model's input.
    #[error(transparent)]
    Preprocess(#[from] PreprocessError),
    /// The saliency model could not be loaded or run.
    #[error(transparent)]
    Saliency(#[from] SaliencyError),
    /// The model's saliency map could not be reduced to a focus point.
    #[error(transparent)]
    Gravity(#[from] GravityError),
    /// The crop around the focus could not be framed or rendered.
    #[error(transparent)]
    Crop(#[from] CropError),
}
