//! What preparing the model input can fail with.

/// A failure fitting a raster into the model's input tensor.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PreprocessError {
    /// The tensor has no pixels.
    #[error("invalid preprocessing dimensions")]
    Dimensions,
    /// The source image has no pixels.
    #[error("invalid image dimensions")]
    Source,
    /// Resampling the image failed.
    #[error("resize image: {0}")]
    Resize(#[from] fast_image_resize::ResizeError),
    /// The raster's pixel buffer does not match its dimensions and layout.
    #[error("resize image: {0}")]
    Buffer(#[from] fast_image_resize::ImageBufferError),
}
