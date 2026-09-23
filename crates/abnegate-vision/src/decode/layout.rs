//! How a raster's samples are laid out.

/// The pixel layout of a decoded raster.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Layout {
    Rgb,
    Rgba,
}

impl Layout {
    pub const fn channels(self) -> usize {
        match self {
            Self::Rgb => 3,
            Self::Rgba => 4,
        }
    }
}
