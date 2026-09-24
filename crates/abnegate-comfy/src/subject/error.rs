use abnegate_vision::CropError;
use abnegate_vision::DecodeError;

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    #[error(transparent)]
    Decode(#[from] DecodeError),
    #[error(transparent)]
    Crop(#[from] CropError),
}
