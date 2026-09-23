//! What planning or rendering a crop can fail with.

use crate::crop::region::Region;
use crate::decode::MAX_PIXELS;

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    #[error("image has no pixels")]
    EmptySource,
    #[error("target size must be at least 1x1")]
    EmptyTarget,
    #[error("crop region {region:?} does not lie inside the {}x{} image", .size.0, .size.1)]
    Region { region: Region, size: (u32, u32) },
    #[error("target of {width}x{height} exceeds the {MAX_PIXELS} pixel ceiling")]
    TargetTooLarge { width: u32, height: u32 },
    #[error("render crop: {0}")]
    Resize(#[from] fast_image_resize::ResizeError),
    #[error("render crop: {0}")]
    Buffer(#[from] fast_image_resize::ImageBufferError),
}
