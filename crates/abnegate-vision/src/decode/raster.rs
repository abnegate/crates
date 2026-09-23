//! A decoded image.

use crate::decode::layout::Layout;
use crate::decode::orientation::Orientation;

/// A decoded image and the orientation that still has to be applied to it.
pub struct Raster {
    pub width: u32,
    pub height: u32,
    pub layout: Layout,
    pub orientation: Orientation,
    pub pixels: Vec<u8>,
}

impl Raster {
    /// The dimensions the image has once its orientation is applied.
    pub const fn oriented_size(&self) -> (u32, u32) {
        if self.orientation.swaps_axes() {
            (self.height, self.width)
        } else {
            (self.width, self.height)
        }
    }
}
