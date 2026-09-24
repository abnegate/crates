use abnegate_vision::CropError;
use abnegate_vision::DecodeError;

/// Why [`Subject::crop`](crate::subject::Subject::crop) or
/// [`Subject::render`](crate::subject::Subject::render) produced no crop.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SubjectError {
    /// The image could not be decoded into a raster.
    #[error(transparent)]
    Decode(#[from] DecodeError),
    /// The decoded raster could not be cropped to the requested square.
    #[error(transparent)]
    Crop(#[from] CropError),
}
