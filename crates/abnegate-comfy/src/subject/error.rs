use abnegate_vision::CropError;
use abnegate_vision::DecodeError;

/// Why [`Subject::crop`](crate::subject::Subject::crop) or
/// [`Subject::render`](crate::subject::Subject::render) produced no crop.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SubjectError {
    #[error(transparent)]
    Decode(#[from] DecodeError),
    #[error(transparent)]
    Crop(#[from] CropError),
}
