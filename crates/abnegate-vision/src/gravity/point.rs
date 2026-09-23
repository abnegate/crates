//! A position in an image.

use serde::Serialize;

/// A normalized coordinate in an image.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}
