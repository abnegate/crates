//! A crop and the decision behind it.

use crate::analyzer::focus::Focus;
use crate::crop::{Region, Rendered};

/// A subject-aware crop and the decision behind it.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Crop {
    /// The rendered crop.
    pub image: Rendered,
    /// Where the subject was found.
    pub focus: Focus,
    /// The region of the oriented source image the crop covers.
    pub region: Region,
    /// The oriented size of the source image.
    pub source: (u32, u32),
}
