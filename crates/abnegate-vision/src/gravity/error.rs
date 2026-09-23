//! What reducing a saliency map to a point can fail with.

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    #[error("invalid saliency map dimensions")]
    Dimensions,
    #[error("invalid saliency map region")]
    Region,
}
