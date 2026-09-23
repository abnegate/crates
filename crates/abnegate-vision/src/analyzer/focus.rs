//! Where a subject sits.

use serde::Serialize;

use crate::gravity::Point;

/// Where an image's visual subject sits, normalized to the oriented image.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[non_exhaustive]
pub struct Focus {
    pub point: Point,
    /// The model's peak activation, in `[0, 1]`.
    ///
    /// This is not a calibrated probability that the subject is the right one;
    /// treat it as a weak signal for routing images to review, not as truth.
    pub confidence: f64,
}
