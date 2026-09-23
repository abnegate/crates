//! Focal-point calculation from saliency maps.

use serde::Serialize;

/// A normalized coordinate in an image.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

/// A half-open rectangle in saliency-map space.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub min_x: i32,
    pub min_y: i32,
    pub max_x: i32,
    pub max_y: i32,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum Error {
    #[error("invalid saliency map dimensions")]
    Dimensions,
    #[error("invalid saliency map region")]
    Region,
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

    const fn contained_by(&self, outer: &Rect) -> bool {
        self.min_x >= outer.min_x
            && self.min_y >= outer.min_y
            && self.max_x <= outer.max_x
            && self.max_y <= outer.max_y
    }
}

/// Returns the saliency-weighted centroid and the peak saliency value as
/// confidence. Values that are negative, NaN, or infinite contribute no weight.
/// A map with no usable saliency falls back to the image center.
pub fn from_saliency(saliency: &[f32], width: i32, height: i32) -> Result<(Point, f64), Error> {
    from_saliency_region(saliency, width, height, Rect::new(0, 0, width, height))
}

/// Calculates a focal point from a rectangular image region within a larger
/// saliency map, ignoring any surrounding letterbox padding.
pub fn from_saliency_region(
    saliency: &[f32],
    width: i32,
    height: i32,
    region: Rect,
) -> Result<(Point, f64), Error> {
    if width <= 0 || height <= 0 || saliency.len() as i64 != i64::from(width) * i64::from(height) {
        return Err(Error::Dimensions);
    }
    if region.is_empty() || !region.contained_by(&Rect::new(0, 0, width, height)) {
        return Err(Error::Region);
    }

    let (total, weighted_x, weighted_y, peak) = accumulate(saliency, width as usize, region);

    let confidence = clamp01(peak as f64);
    if total == 0.0 {
        return Ok((Point { x: 0.5, y: 0.5 }, confidence));
    }

    let mut x = 0.5;
    let mut y = 0.5;
    if region.width() > 1 {
        x = weighted_x / total / f64::from(region.width() - 1);
    }
    if region.height() > 1 {
        y = weighted_y / total / f64::from(region.height() - 1);
    }

    Ok((
        Point {
            x: clamp01(x),
            y: clamp01(y),
        },
        confidence,
    ))
}

/// Accumulates the region's weight sum, first moments, and peak in one pass.
///
/// Row sums are folded once per row rather than once per pixel, which keeps the
/// inner loop free of loop-carried multiplies so it vectorizes.
fn accumulate(saliency: &[f32], width: usize, region: Rect) -> (f64, f64, f64, f32) {
    let mut total = 0.0f64;
    let mut weighted_x = 0.0f64;
    let mut weighted_y = 0.0f64;
    let mut peak = 0.0f32;

    for y in region.min_y..region.max_y {
        let start = y as usize * width + region.min_x as usize;
        let row = &saliency[start..start + region.width() as usize];

        let mut row_total = 0.0f64;
        let mut row_weighted_x = 0.0f64;
        let mut row_peak = 0.0f32;
        for (x, &value) in row.iter().enumerate() {
            let weight = if value.is_finite() {
                value.max(0.0)
            } else {
                0.0
            };
            row_total += f64::from(weight);
            row_weighted_x += x as f64 * f64::from(weight);
            row_peak = row_peak.max(weight);
        }

        total += row_total;
        weighted_x += row_weighted_x;
        weighted_y += f64::from(y - region.min_y) * row_total;
        peak = peak.max(row_peak);
    }

    (total, weighted_x, weighted_y, peak)
}

fn clamp01(value: f64) -> f64 {
    value.clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_point_at_top_right() {
        let map = [0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        let (point, confidence) = from_saliency(&map, 3, 3).unwrap();
        assert_eq!(point, Point { x: 1.0, y: 0.0 });
        assert_eq!(confidence, 1.0);
    }

    #[test]
    fn weighted_centroid() {
        let (point, confidence) = from_saliency(&[1.0, 0.0, 0.0, 3.0], 2, 2).unwrap();
        assert_eq!(point, Point { x: 0.75, y: 0.75 });
        assert_eq!(confidence, 1.0);
    }

    #[test]
    fn empty_map_falls_back_to_center() {
        let (point, confidence) = from_saliency(&[0.0; 8], 4, 2).unwrap();
        assert_eq!(point, Point { x: 0.5, y: 0.5 });
        assert_eq!(confidence, 0.0);
    }

    #[test]
    fn rejects_invalid_dimensions() {
        assert_eq!(from_saliency(&[1.0], 2, 2), Err(Error::Dimensions));
    }

    #[test]
    fn region_ignores_padding() {
        const WIDTH: usize = 4;
        let mut map = [0.0f32; WIDTH * WIDTH];
        map[0] = 10.0;
        map[WIDTH + 3] = 0.5;

        let (point, confidence) = from_saliency_region(&map, 4, 4, Rect::new(0, 1, 4, 3)).unwrap();
        assert_eq!(point, Point { x: 1.0, y: 0.0 });
        assert_eq!(confidence, 0.5);
    }

    #[test]
    fn rejects_invalid_region() {
        let error = from_saliency_region(&[0.0; 4], 2, 2, Rect::new(-1, 0, 1, 1));
        assert_eq!(error, Err(Error::Region));
    }

    #[test]
    fn non_finite_and_negative_weights_are_ignored() {
        let map = [f32::NAN, f32::INFINITY, -5.0, 2.0];
        let (point, confidence) = from_saliency(&map, 2, 2).unwrap();
        assert_eq!(point, Point { x: 1.0, y: 1.0 });
        assert_eq!(confidence, 1.0);
    }
}
