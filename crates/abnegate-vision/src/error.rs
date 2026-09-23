//! What decoding and cropping can fail with, without the model.

use crate::{crop, decode};

/// A failure from [`decode`](crate::decode) or [`crop`](crate::crop), for a
/// caller that frames crops itself and wants one error type across both.
///
/// With the `saliency` feature, `AnalyzerError` covers the model as well.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    #[error(transparent)]
    Decode(#[from] decode::Error),
    #[error(transparent)]
    Crop(#[from] crop::Error),
}
