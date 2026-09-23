use abnegate_vision::crop;
use abnegate_vision::decode;

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    #[error(transparent)]
    Decode(#[from] decode::Error),
    #[error(transparent)]
    Crop(#[from] crop::Error),
}
