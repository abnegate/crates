//! What decoding can fail with.

/// A failure decoding an image.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum DecodeError {
    /// The data is not JPEG, PNG or WebP.
    #[error("unsupported image format: unknown image format")]
    UnknownFormat,
    /// The image has no pixels, or more than
    /// [`MAXIMUM_PIXELS`](crate::decode::MAXIMUM_PIXELS).
    #[error("image dimensions are too large")]
    TooLarge,
    /// The decoder refused the data, or would have allocated past its budget.
    #[error("decode image: {0}")]
    Decode(#[from] image::ImageError),
}
