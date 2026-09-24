//! A crop, rendered.

/// A rendered crop: 8-bit RGB, row-major, `width * height * 3` bytes.
#[derive(Debug, Clone)]
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
