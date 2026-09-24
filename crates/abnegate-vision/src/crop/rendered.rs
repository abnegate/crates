//! A crop, rendered.

use std::fmt;

/// A rendered crop: 8-bit RGB, row-major, `width * height * 3` bytes.
///
/// `Debug` reports the pixel buffer by its length, not its samples.
#[derive(Clone)]
#[non_exhaustive]
pub struct Rendered {
    /// The image's width, in pixels.
    pub width: u32,
    /// The image's height, in pixels.
    pub height: u32,
    /// The RGB samples, row by row.
    pub pixels: Vec<u8>,
}

impl Rendered {
    /// A `width` x `height` image from its row-major RGB samples.
    pub const fn new(width: u32, height: u32, pixels: Vec<u8>) -> Self {
        Self {
            width,
            height,
            pixels,
        }
    }
}

impl fmt::Debug for Rendered {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Rendered")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("pixels", &format_args!("{} bytes", self.pixels.len()))
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_reports_the_pixels_by_length() {
        let rendered = Rendered::new(4, 2, vec![7; 4 * 2 * 3]);
        assert_eq!(
            format!("{rendered:?}"),
            "Rendered { width: 4, height: 2, pixels: 24 bytes }"
        );
    }
}
