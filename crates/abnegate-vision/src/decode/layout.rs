//! How a raster's samples are laid out.

/// The pixel layout of a decoded raster.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Layout {
    /// Three 8-bit samples per pixel: red, green, blue.
    Rgb,
    /// Four 8-bit samples per pixel: red, green, blue, alpha.
    Rgba,
}

impl Layout {
    /// Samples per pixel: 3 for [`Rgb`](Self::Rgb), 4 for
    /// [`Rgba`](Self::Rgba).
    pub const fn channels(self) -> usize {
        match self {
            Self::Rgb => 3,
            Self::Rgba => 4,
        }
    }
}
