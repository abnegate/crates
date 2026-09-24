//! A decoded image.

use std::fmt;

use crate::decode::layout::Layout;
use crate::decode::orientation::Orientation;

/// A decoded image and the orientation that still has to be applied to it.
///
/// `Debug` reports the pixel buffer by its length, not its samples.
#[derive(Clone)]
#[non_exhaustive]
pub struct Raster {
    /// The stored width, before orientation, in pixels.
    pub width: u32,
    /// The stored height, before orientation, in pixels.
    pub height: u32,
    /// How each pixel's samples are laid out.
    pub layout: Layout,
    /// The EXIF orientation still to be applied.
    pub orientation: Orientation,
    /// The samples, row by row, in `layout`.
    pub pixels: Vec<u8>,
}

impl Raster {
    /// An upright `width` x `height` image from its row-major samples.
    pub const fn new(width: u32, height: u32, layout: Layout, pixels: Vec<u8>) -> Self {
        Self {
            width,
            height,
            layout,
            orientation: Orientation::Normal,
            pixels,
        }
    }

    /// The same image, with `orientation` still to be applied to it.
    pub const fn with_orientation(mut self, orientation: Orientation) -> Self {
        self.orientation = orientation;
        self
    }

    /// The dimensions the image has once its orientation is applied.
    pub const fn oriented_size(&self) -> (u32, u32) {
        if self.orientation.swaps_axes() {
            (self.height, self.width)
        } else {
            (self.width, self.height)
        }
    }
}

impl fmt::Debug for Raster {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Raster")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("layout", &self.layout)
            .field("orientation", &self.orientation)
            .field("pixels", &format_args!("{} bytes", self.pixels.len()))
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_reports_the_pixels_by_length() {
        let raster = Raster::new(4, 2, Layout::Rgb, vec![7; 4 * 2 * 3]);
        assert_eq!(
            format!("{raster:?}"),
            "Raster { width: 4, height: 2, layout: Rgb, orientation: Normal, pixels: 24 bytes }"
        );
    }

    #[test]
    fn a_clone_is_the_same_image() {
        let raster = Raster::new(2, 1, Layout::Rgb, vec![1, 2, 3, 4, 5, 6])
            .with_orientation(Orientation::Rotate90);
        let copy = raster.clone();
        assert_eq!(copy.width, raster.width);
        assert_eq!(copy.height, raster.height);
        assert_eq!(copy.layout, raster.layout);
        assert_eq!(copy.orientation, raster.orientation);
        assert_eq!(copy.pixels, raster.pixels);
    }

    #[test]
    fn a_new_raster_is_upright_until_given_an_orientation() {
        let upright = Raster::new(4, 2, Layout::Rgb, vec![0; 4 * 2 * 3]);
        assert_eq!(upright.orientation, Orientation::Normal);
        assert_eq!(upright.oriented_size(), (4, 2));

        let sideways = upright.with_orientation(Orientation::Rotate90);
        assert_eq!(sideways.orientation, Orientation::Rotate90);
        assert_eq!(sideways.oriented_size(), (2, 4));
    }
}
