//! The frame a crop is rendered to.

/// The output frame a caller wants, in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Target {
    /// The frame's width, in pixels.
    pub width: u32,
    /// The frame's height, in pixels.
    pub height: u32,
}

impl Target {
    /// A `width` x `height` frame.
    pub const fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }

    /// The square frame most training pipelines want.
    pub const fn square(size: u32) -> Self {
        Self::new(size, size)
    }

    pub(crate) fn aspect(self) -> f64 {
        f64::from(self.width) / f64::from(self.height)
    }

    pub(crate) const fn area(self) -> u64 {
        self.width as u64 * self.height as u64
    }
}
