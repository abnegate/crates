//! A crop and the decision behind it.

use crate::analyzer::focus::Focus;
use crate::crop::{Region, Rendered};

/// A subject-aware crop and the decision behind it.
#[derive(Debug, Clone)]
pub struct Crop {
    pub image: Rendered,
    pub focus: Focus,
    pub region: Region,
    /// The oriented size of the source image.
    pub source: (u32, u32),
}
