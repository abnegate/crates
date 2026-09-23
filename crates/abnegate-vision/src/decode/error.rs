//! What decoding can fail with.

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    #[error("unsupported image format: unknown image format")]
    UnknownFormat,
    #[error("image dimensions are too large")]
    TooLarge,
    #[error("decode image: {0}")]
    Decode(#[from] image::ImageError),
}
