//! What preparing the model input can fail with.

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    #[error("invalid preprocessing dimensions")]
    Dimensions,
    #[error("invalid image dimensions")]
    Source,
    #[error("resize image: {0}")]
    Resize(#[from] fast_image_resize::ResizeError),
    #[error("resize image: {0}")]
    Buffer(#[from] fast_image_resize::ImageBufferError),
}
