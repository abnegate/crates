//! A position in an image.

use serde::Serialize;

/// A normalized coordinate in an image.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Point {
    /// Distance from the left edge as a fraction of the width, in `[0, 1]`.
    pub x: f64,
    /// Distance from the top edge as a fraction of the height, in `[0, 1]`.
    pub y: f64,
}
