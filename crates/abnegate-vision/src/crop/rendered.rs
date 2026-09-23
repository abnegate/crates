//! A crop, rendered.

/// A rendered crop: 8-bit RGB, row-major, `width * height * 3` bytes.
#[derive(Debug, Clone)]
pub struct Rendered {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}
