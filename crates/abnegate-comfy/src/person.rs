//! Person-training crops.
//!
//! A square crop of a body shot loses the head or the feet, so a person's
//! photo keeps its aspect instead: the whole person in a bucket of about a
//! megapixel, snapped to the 64-pixel grid SDXL's latents need, plus a head
//! and shoulders square on the same subject so the face stays sharp, and a
//! square on each hand when the caption says the hands are doing something.

mod frame;

pub(crate) use frame::Frame;

use crate::dataset::FrameKind;
use crate::subject::CENTRE;
use crate::subject::Subject;
use crate::subject::SubjectError;
use abnegate_vision::Raster;
use abnegate_vision::crop;
use abnegate_vision::crop::Region;
use abnegate_vision::crop::Target;
use abnegate_vision::gravity::Point;

/// The side a person's photo is screened against, the side of the square
/// whose area every body bucket keeps.
pub(crate) const SIDE: u32 = 1024;
const SNAP: u32 = 64;
const MINIMUM_SIDE: u32 = 512;
const MAXIMUM_SIDE: u32 = 1536;
const HAND_WORDS: &[&str] = &[
    "hand", "hands", "finger", "fingers", "fist", "fists", "palm", "palms",
];

/// Every crop a person's photo `raster`, captioned `caption`, contributes to
/// the set: the body, the head unless the body is already that square, and
/// one crop per distinct hand frame when the caption names hands.
pub(crate) fn frames(
    subject: &Subject,
    raster: &Raster,
    caption: &str,
) -> Result<Vec<Frame>, SubjectError> {
    let size = raster.oriented_size();
    let located = subject.locate(raster, CENTRE);
    let body = bucket(size.0, size.1);
    let close = close_target(body);
    let mut placements = vec![(FrameKind::Body, body, located.focus)];
    if !is_duplicate(body, close) {
        placements.push((FrameKind::Head, close, head_focus(located.focus)));
        if mentions_hands(caption) {
            for left in [true, false] {
                placements.push((FrameKind::Hand, close, hand_focus(located.focus, left)));
            }
        }
    }
    let mut hands: Vec<Region> = Vec::new();
    let mut frames = Vec::with_capacity(placements.len());
    for (kind, target, focus) in placements {
        let region = crop::plan(size, target, focus)?;
        if kind == FrameKind::Hand {
            if hands.contains(&region) {
                continue;
            }
            hands.push(region);
        }
        frames.push(Frame {
            kind,
            rendered: crop::render(raster, region, target)?,
            mask: located.mask(size, region, target),
        });
    }
    Ok(frames)
}

/// The body bucket of a `width` x `height` photo: its aspect, an area close
/// to [`SIDE`] squared, both sides on the 64-pixel grid.
pub(crate) fn bucket(width: u32, height: u32) -> Target {
    let aspect = f64::from(width.max(1)) / f64::from(height.max(1));
    let height = (f64::from(SIDE * SIDE) / aspect).sqrt();
    Target::new(snap(height * aspect), snap(height))
}

/// The head and hand square of a `body` bucket: as wide as its shorter side,
/// so a square close-up is its own head crop.
pub(crate) fn close_target(body: Target) -> Target {
    Target::square(snap(f64::from(body.width.min(body.height))))
}

/// Moves the subject's centre of mass, which sits on a standing person's
/// torso, up to the head.
pub(crate) fn head_focus(focus: Point) -> Point {
    Point {
        x: focus.x,
        y: (focus.y * 0.55).clamp(0.18, 0.42),
    }
}

/// Whether a close-up square would frame the same thing as the body.
pub(crate) fn is_duplicate(body: Target, close: Target) -> bool {
    body == close
}

fn hand_focus(focus: Point, left: bool) -> Point {
    Point {
        x: if left {
            (focus.x - 0.18).clamp(0.12, 0.45)
        } else {
            (focus.x + 0.18).clamp(0.55, 0.88)
        },
        y: (focus.y + 0.22).clamp(0.58, 0.88),
    }
}

fn mentions_hands(caption: &str) -> bool {
    caption
        .split(|character: char| !character.is_alphanumeric())
        .any(|word| {
            HAND_WORDS
                .iter()
                .any(|hand| hand.eq_ignore_ascii_case(word))
        })
}

fn snap(value: f64) -> u32 {
    let snapped = ((value / f64::from(SNAP)).round() * f64::from(SNAP)).max(f64::from(SNAP));
    (snapped as u32).clamp(MINIMUM_SIDE, MAXIMUM_SIDE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use abnegate_vision::decode::Layout;

    fn raster(width: u32, height: u32) -> Raster {
        Raster::new(
            width,
            height,
            Layout::Rgb,
            vec![80; (width * height * 3) as usize],
        )
    }

    fn kinds(frames: &[Frame]) -> Vec<FrameKind> {
        frames.iter().map(|frame| frame.kind).collect()
    }

    #[test]
    fn a_portrait_stays_taller_than_it_is_wide() {
        let target = bucket(768, 1024);
        assert_eq!(target.width % 64, 0);
        assert_eq!(target.height % 64, 0);
        assert!(target.height > target.width);
        let area = u64::from(target.width) * u64::from(target.height);
        assert!(area > 700_000 && area < 1_400_000, "area {area}");
    }

    #[test]
    fn a_landscape_stays_wider_than_it_is_tall() {
        let target = bucket(1600, 900);
        assert!(target.width > target.height);
        assert_eq!(target.width % 64, 0);
        assert_eq!(target.height % 64, 0);
    }

    #[test]
    fn an_extreme_aspect_stays_inside_the_side_limits() {
        for (width, height) in [(1, 10_000), (10_000, 1), (0, 0)] {
            let target = bucket(width, height);
            for side in [target.width, target.height] {
                assert!((MINIMUM_SIDE..=MAXIMUM_SIDE).contains(&side), "{target:?}");
                assert_eq!(side % SNAP, 0);
            }
        }
    }

    #[test]
    fn a_square_close_up_does_not_emit_a_second_head_crop() {
        let body = bucket(1024, 1024);
        assert!(is_duplicate(body, close_target(body)));
    }

    #[test]
    fn a_full_body_portrait_gets_a_square_head_crop() {
        let body = bucket(768, 1344);
        let head = close_target(body);
        assert!(!is_duplicate(body, head));
        assert_eq!(head.width, head.height);
        assert!(head.width >= 512);
    }

    #[test]
    fn head_focus_moves_up_from_the_torso() {
        let focus = head_focus(Point { x: 0.5, y: 0.62 });
        assert_eq!(focus.x, 0.5);
        assert!(focus.y < 0.62);
        assert!(focus.y >= 0.18);
    }

    #[test]
    fn hands_are_named_as_whole_words() {
        assert!(mentions_hands("standing with Hands on hips"));
        assert!(mentions_hands("a raised fist"));
        assert!(!mentions_hands("handstand on a beach"));
        assert!(!mentions_hands("standing outside"));
    }

    #[test]
    fn a_body_shot_is_framed_whole_and_at_the_head() {
        let frames = frames(&Subject::none(), &raster(48, 84), "standing outside").unwrap();
        assert_eq!(kinds(&frames), [FrameKind::Body, FrameKind::Head]);
        let (body, head) = (&frames[0].rendered, &frames[1].rendered);
        assert!(body.height > body.width);
        assert_eq!(head.width, head.height);
        assert!(
            frames.iter().all(|frame| frame.mask.is_none()),
            "no model means no mask"
        );
    }

    #[test]
    fn a_tight_face_square_does_not_emit_a_head_or_hand_crop() {
        let frames = frames(&Subject::none(), &raster(64, 64), "hands on hips, close-up").unwrap();
        assert_eq!(
            kinds(&frames),
            [FrameKind::Body],
            "a square close-up is already the head and hand frame"
        );
    }

    #[test]
    fn a_full_body_shot_that_names_hands_gets_a_hand_crop() {
        let frames = frames(
            &Subject::none(),
            &raster(48, 84),
            "standing with hands on hips",
        )
        .unwrap();
        assert_eq!(
            kinds(&frames),
            [FrameKind::Body, FrameKind::Head, FrameKind::Hand],
            "a portrait cannot slide left vs right, so its two hand frames are one"
        );
        let hand = &frames[2].rendered;
        assert_eq!(hand.width, hand.height);
    }

    #[test]
    fn a_wide_shot_that_names_hands_gets_one_crop_per_hand() {
        let frames = frames(&Subject::none(), &raster(300, 100), "hands raised").unwrap();
        assert_eq!(
            kinds(&frames),
            [
                FrameKind::Body,
                FrameKind::Head,
                FrameKind::Hand,
                FrameKind::Hand
            ]
        );
    }
}
