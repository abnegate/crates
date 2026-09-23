//! The frame a crop is rendered to.

/// The output frame a caller wants, in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Target {
    pub width: u32,
    pub height: u32,
}

impl Target {
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
