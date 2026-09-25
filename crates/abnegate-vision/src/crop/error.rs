//! What planning or rendering a crop can fail with.

use crate::crop::region::Region;
use crate::decode::MAXIMUM_PIXELS;

/// A failure planning or rendering a crop.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum CropError {
    /// The source image has no pixels.
    #[error("image has no pixels")]
    EmptySource,
    /// The target frame has no pixels.
    #[error("target size must be at least 1x1")]
    EmptyTarget,
    /// The region is empty or reaches outside the oriented source image.
    #[error("crop region {region:?} does not lie inside the {}x{} image", .size.0, .size.1)]
    #[non_exhaustive]
    Region {
        /// The region asked for.
        region: Region,
        /// The oriented size of the source image.
        size: (u32, u32),
    },
    /// The target frame holds more than [`MAXIMUM_PIXELS`].
    #[error("target of {width}x{height} exceeds the {MAXIMUM_PIXELS} pixel ceiling")]
    #[non_exhaustive]
    TargetTooLarge {
        /// The target's width.
        width: u32,
        /// The target's height.
        height: u32,
    },
    /// Resampling the region failed.
    #[error("render crop: {0}")]
    Resize(#[from] fast_image_resize::ResizeError),
    /// The raster's pixel buffer does not match its dimensions and layout.
    #[error("render crop: {0}")]
    Buffer(#[from] fast_image_resize::ImageBufferError),
}
