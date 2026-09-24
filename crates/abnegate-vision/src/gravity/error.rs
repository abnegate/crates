//! What reducing a saliency map to a point can fail with.

/// A failure reducing a saliency map to a focal point.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum GravityError {
    /// The map does not hold `width * height` values, or a side is not
    /// positive.
    #[error("invalid saliency map dimensions")]
    Dimensions,
    /// The region is empty or reaches outside the map.
    #[error("invalid saliency map region")]
    Region,
}
