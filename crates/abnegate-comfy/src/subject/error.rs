use abnegate_vision::crop;
use abnegate_vision::decode;

/// Why [`Subject::crop`](crate::subject::Subject::crop) or
/// [`Subject::render`](crate::subject::Subject::render) produced no crop.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SubjectError {
    #[error(transparent)]
    Decode(#[from] decode::Error),
    #[error(transparent)]
    Crop(#[from] crop::Error),
}
