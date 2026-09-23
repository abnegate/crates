//! A rectangle in saliency-map space.

/// A half-open rectangle in saliency-map space.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub min_x: i32,
    pub min_y: i32,
    pub max_x: i32,
    pub max_y: i32,
}

impl Rect {
    pub const fn new(min_x: i32, min_y: i32, max_x: i32, max_y: i32) -> Self {
        Self {
            min_x,
            min_y,
            max_x,
            max_y,
        }
    }

    pub const fn width(&self) -> i32 {
        self.max_x - self.min_x
    }

    pub const fn height(&self) -> i32 {
        self.max_y - self.min_y
    }

    pub const fn is_empty(&self) -> bool {
        self.min_x >= self.max_x || self.min_y >= self.max_y
    }

    pub(crate) const fn contained_by(&self, outer: &Rect) -> bool {
        self.min_x >= outer.min_x
            && self.min_y >= outer.min_y
            && self.max_x <= outer.max_x
            && self.max_y <= outer.max_y
    }
}
