//! A rectangle of the source image.

use serde::Serialize;

/// A region of the oriented source image, in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Region {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl Region {
    /// Whether this is a non-empty region lying wholly inside an image of `size`.
    pub(crate) fn lies_within(self, size: (u32, u32)) -> bool {
        let spans = |start: u32, extent: u32, limit: u32| {
            extent > 0 && start.checked_add(extent).is_some_and(|end| end <= limit)
        };
        spans(self.x, self.width, size.0) && spans(self.y, self.height, size.1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn region(x: u32, y: u32, width: u32, height: u32) -> Region {
        Region {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn a_region_inside_the_image_lies_within_it() {
        assert!(region(0, 0, 80, 60).lies_within((80, 60)));
        assert!(region(79, 59, 1, 1).lies_within((80, 60)));
    }

    #[test]
    fn an_empty_overflowing_or_overhanging_region_does_not() {
        for rejected in [
            region(0, 0, 0, 10),
            region(0, 0, 10, 0),
            region(u32::MAX, 0, 2, 1),
            region(0, u32::MAX, 1, 2),
            region(71, 0, 10, 10),
            region(0, 51, 10, 10),
            region(80, 0, 1, 1),
        ] {
            assert!(!rejected.lies_within((80, 60)), "{rejected:?}");
        }
    }
}
