//! A rectangle in saliency-map space.

/// A half-open rectangle in saliency-map space: `left` and `top` are the first
/// column and row inside it, `right` and `bottom` the first ones past it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rectangle {
    /// The first column inside the rectangle.
    pub left: i32,
    /// The first row inside the rectangle.
    pub top: i32,
    /// The first column past the rectangle's right edge.
    pub right: i32,
    /// The first row past the rectangle's bottom edge.
    pub bottom: i32,
}

impl Rectangle {
    /// The rectangle spanning columns `left..right` and rows `top..bottom`.
    pub const fn new(left: i32, top: i32, right: i32, bottom: i32) -> Self {
        Self {
            left,
            top,
            right,
            bottom,
        }
    }

    /// The number of columns the rectangle spans.
    pub const fn width(&self) -> i32 {
        self.right - self.left
    }

    /// The number of rows the rectangle spans.
    pub const fn height(&self) -> i32 {
        self.bottom - self.top
    }

    /// Whether the rectangle covers no cell at all.
    pub const fn is_empty(&self) -> bool {
        self.left >= self.right || self.top >= self.bottom
    }

    pub(crate) const fn contained_by(&self, outer: &Rectangle) -> bool {
        self.left >= outer.left
            && self.top >= outer.top
            && self.right <= outer.right
            && self.bottom <= outer.bottom
    }
}
