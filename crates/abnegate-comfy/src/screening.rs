//! Screening a training set before the run starts.
//!
//! [`crate::dataset::inspect`] reads what the captioner said about a set; this
//! reads the pixels. Both answer the same question — can this set teach one
//! subject — from independent evidence, so a burst the captioner happened to
//! describe eight different ways is still caught here.
//!
//! Three things are worth acting on. Near-identical frames train the adapter to
//! reproduce one pose, and a perceptual hash finds them. A frame the camera
//! smeared teaches blur as part of the subject. An image well under the
//! training resolution is upscaled into softness.
//!
//! Unlike `inspect`, this acts: the caller trains on [`Verdict::keep`] and tells
//! the user what [`Verdict::drop`] cost them, the same way auto-crop and
//! auto-caption fix a set rather than filing a complaint about it. Two rules
//! bound that. Nothing that fails to decode is ever dropped, because screening
//! is the wrong place to fail a run. And the set never falls below five images:
//! when it would, the most salvageable rejects come back, since returning four
//! images and a clean conscience is worse than returning eight and a warning.

use abnegate_vision::crop::{self, Region, Target};
use abnegate_vision::decode::{self, Raster};
use serde::Serialize;

/// Matches `dataset::inspect`'s own minimum: a measured eight-image run
/// improved its subject by 34.72%, and five is the point below which a set
/// stops being able to teach one at all.
const FLOOR: usize = 5;
/// Long edge every image is reduced to before it is measured. Both measures are
/// scale-dependent, so a fixed analysis size is what lets one threshold hold
/// for a phone panorama and a thumbnail alike. Images already smaller are left
/// alone rather than upscaled into detail they never had.
const WORKING: u32 = 256;
/// Hamming distance between two 64-bit fingerprints at or below which the
/// frames are the same shot. Re-encodes, blurs and small shifts of one picture
/// measured 0 to 8; moving the camera by 1% of the frame measured 1 to 9, by 5%
/// measured 9 to 24, and unrelated subjects 23 to 41.
const DISTANCE: u32 = 10;
/// How much blurrier than the rest of the set a frame has to be. There is no
/// absolute threshold to find: measured across gradients, textures, photographs
/// and screenshots, focus moves a blur score by two to five times and content
/// moves it by eighty, so the only stable reference is the set itself, which is
/// one subject shot by one camera. Against that reference, other framings of
/// the same subject measured 0.94 to 1.21, soft focus 1.04 to 1.17, and a
/// shallow depth of field holding the subject on 60% of the frame 0.97 to 1.21;
/// a motion smear across 5% of the frame measured 1.37 to 1.70.
const SOFTER: f64 = 1.30;
/// Enlargement the training resize is expected to absorb. Past it the image is
/// mostly interpolation.
const UPSCALE: f64 = 2.0;
/// Span of the low-pass the blur measure compares against, in working pixels.
const WINDOW: usize = 9;
/// Blur is measured across and down, never pooled: motion ruins one direction.
const AXES: usize = 2;
const FINGERPRINT_WIDTH: usize = 9;
const FINGERPRINT_HEIGHT: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Rejection {
    Duplicate,
    Blurred,
    Small,
}

/// Which images to train on, and what the rest were rejected for. The two lists
/// partition the caller's slice: an image restored to keep the set at its floor
/// appears in `keep`, not in `drop`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Verdict {
    pub keep: Vec<usize>,
    pub drop: Vec<(usize, Rejection)>,
}

/// Screen a training set, keeping the images worth an hour of training.
///
/// `resolution` is the resolution the images will be trained at. Indices address
/// `images`.
pub fn screen(images: &[Vec<u8>], resolution: u32) -> Verdict {
    let samples: Vec<Option<Sample>> = images.iter().map(|bytes| measure(bytes)).collect();
    let baseline = baseline(&samples);

    let mut keep: Vec<usize> = Vec::with_capacity(images.len());
    let mut kept: Vec<u64> = Vec::with_capacity(images.len());
    let mut rejected: Vec<Rejected> = Vec::new();
    for (index, sample) in samples.iter().enumerate() {
        let Some(sample) = *sample else {
            keep.push(index);
            continue;
        };
        let softness = softness(sample.blur, baseline);
        let rejection = if f64::from(sample.shortest) * UPSCALE < f64::from(resolution) {
            Rejection::Small
        } else if softness > SOFTER {
            Rejection::Blurred
        } else if kept
            .iter()
            .any(|hash| distance(*hash, sample.hash) <= DISTANCE)
        {
            Rejection::Duplicate
        } else {
            kept.push(sample.hash);
            keep.push(index);
            continue;
        };
        rejected.push(Rejected {
            index,
            rejection,
            softness,
            hash: sample.hash,
        });
    }

    let floor = FLOOR.min(images.len());
    while keep.len() < floor && !rejected.is_empty() {
        let restored = rejected.remove(salvageable(&rejected, &kept));
        kept.push(restored.hash);
        keep.push(restored.index);
    }

    keep.sort_unstable();
    let mut drop: Vec<(usize, Rejection)> = rejected
        .into_iter()
        .map(|entry| (entry.index, entry.rejection))
        .collect();
    drop.sort_unstable_by_key(|(index, _)| *index);
    Verdict { keep, drop }
}

struct Rejected {
    index: usize,
    rejection: Rejection,
    softness: f64,
    hash: u64,
}

#[derive(Clone, Copy)]
struct Sample {
    shortest: u32,
    blur: [f64; AXES],
    hash: u64,
}

struct Greyscale {
    pixels: Vec<f32>,
    width: usize,
    height: usize,
}

/// The blur every frame is judged against, per axis: the lower middle of the
/// set, so an even split still reads from the sharper half. Infinite when
/// nothing decoded, which leaves an unmeasurable set with nothing to compare.
fn baseline(samples: &[Option<Sample>]) -> [f64; AXES] {
    std::array::from_fn(|axis| {
        let mut blurs: Vec<f64> = samples
            .iter()
            .flatten()
            .map(|sample| sample.blur[axis])
            .collect();
        blurs.sort_unstable_by(f64::total_cmp);
        blurs
            .get(blurs.len().saturating_sub(1) / 2)
            .copied()
            .unwrap_or(f64::INFINITY)
    })
}

/// How much blurrier than the set this frame is, on whichever axis has suffered
/// most. Comparing each axis against the set's own baseline for that axis is
/// what makes a sideways smear visible: it ruins one direction and leaves the
/// other alone, so a single figure for the frame hides most of it behind the
/// untouched axis.
fn softness(blur: [f64; AXES], baseline: [f64; AXES]) -> f64 {
    (0..AXES)
        .map(|axis| blur[axis] / baseline[axis].max(f64::EPSILON))
        .fold(0.0, f64::max)
}

/// The reject the set can least afford to lose. A duplicate costs the run
/// nothing but repetition, so those come back before anything that would teach
/// softness; among equals, the frame furthest from what is already kept, then
/// the sharpest.
fn salvageable(rejected: &[Rejected], kept: &[u64]) -> usize {
    let mut position = 0;
    let mut best = salvage(&rejected[0], kept);
    for (candidate, entry) in rejected.iter().enumerate().skip(1) {
        let score = salvage(entry, kept);
        if score > best {
            position = candidate;
            best = score;
        }
    }
    position
}

fn salvage(entry: &Rejected, kept: &[u64]) -> (bool, u32, f64) {
    let furthest = kept
        .iter()
        .map(|hash| distance(*hash, entry.hash))
        .min()
        .unwrap_or(u64::BITS);
    (
        entry.rejection == Rejection::Duplicate,
        furthest,
        -entry.softness,
    )
}

const fn distance(left: u64, right: u64) -> u32 {
    (left ^ right).count_ones()
}

fn measure(bytes: &[u8]) -> Option<Sample> {
    let raster = decode::decode(bytes).ok()?;
    let (width, height) = raster.oriented_size();
    let greyscale = reduce(&raster)?;
    Some(Sample {
        shortest: width.min(height),
        blur: blur(&greyscale),
        hash: fingerprint(&greyscale),
    })
}

/// Applies EXIF orientation, reduces the long edge to [`WORKING`], and flattens
/// to luma. Both measures read this one buffer.
fn reduce(raster: &Raster) -> Option<Greyscale> {
    let (width, height) = raster.oriented_size();
    let long = width.max(height);
    if long == 0 {
        return None;
    }
    let scale = f64::from(WORKING.min(long)) / f64::from(long);
    let target = Target::new(edge(width, scale), edge(height, scale));
    let region = Region {
        x: 0,
        y: 0,
        width,
        height,
    };
    let rendered = crop::render(raster, region, target).ok()?;
    Some(Greyscale {
        pixels: rendered
            .pixels
            .as_chunks::<3>()
            .0
            .iter()
            .map(|pixel| {
                0.299 * f32::from(pixel[0])
                    + 0.587 * f32::from(pixel[1])
                    + 0.114 * f32::from(pixel[2])
            })
            .collect(),
        width: target.width as usize,
        height: target.height as usize,
    })
}

fn edge(source: u32, scale: f64) -> u32 {
    ((f64::from(source) * scale).round() as u32).max(1)
}

/// How little an extra low-pass changes the image, across and down. 0 is sharp
/// and 1 is featureless: detail a blur can still destroy is detail the camera
/// captured, and a frame already smeared has none left to lose. Crete et al.'s
/// perceptual blur metric, over the Laplacian's territory but as a ratio,
/// because the raw variance of the Laplacian tracks content eighty times harder
/// than it tracks focus.
fn blur(greyscale: &Greyscale) -> [f64; AXES] {
    let (width, height) = (greyscale.width, greyscale.height);
    if width < 3 || height < 3 {
        return [1.0; AXES];
    }
    let pixels = &greyscale.pixels;
    let mut across = vec![0.0f32; pixels.len()];
    let mut down = vec![0.0f32; pixels.len()];
    for y in 0..height {
        for x in 0..width {
            let (mut row, mut column) = (0.0f32, 0.0f32);
            for step in 0..WINDOW {
                row += pixels[y * width + (x + step).saturating_sub(WINDOW / 2).min(width - 1)];
                column += pixels[(y + step).saturating_sub(WINDOW / 2).min(height - 1) * width + x];
            }
            across[y * width + x] = row / WINDOW as f32;
            down[y * width + x] = column / WINDOW as f32;
        }
    }

    let axis = |smoothed: &[f32], step: usize| {
        let (mut present, mut lost) = (0.0f64, 0.0f64);
        for y in 1..height - 1 {
            for x in 1..width - 1 {
                let index = y * width + x;
                let sharp = f64::from((pixels[index] - pixels[index - step]).abs());
                let soft = f64::from((smoothed[index] - smoothed[index - step]).abs());
                present += sharp;
                lost += (sharp - soft).max(0.0);
            }
        }
        if present <= f64::EPSILON {
            1.0
        } else {
            (present - lost) / present
        }
    };
    [axis(&across, 1), axis(&down, width)]
}

/// A difference hash: the image averaged down to nine columns by eight rows,
/// then one bit per neighbouring pair saying which side is brighter. Comparing
/// neighbours rather than absolute levels is what survives a re-encode or an
/// exposure shift while still separating one framing from the next.
fn fingerprint(greyscale: &Greyscale) -> u64 {
    let mut totals = [0.0f64; FINGERPRINT_WIDTH * FINGERPRINT_HEIGHT];
    let mut counts = [0.0f64; FINGERPRINT_WIDTH * FINGERPRINT_HEIGHT];
    for y in 0..greyscale.height {
        let row = y * FINGERPRINT_HEIGHT / greyscale.height;
        for x in 0..greyscale.width {
            let cell = row * FINGERPRINT_WIDTH + x * FINGERPRINT_WIDTH / greyscale.width;
            totals[cell] += f64::from(greyscale.pixels[y * greyscale.width + x]);
            counts[cell] += 1.0;
        }
    }

    let mut hash = 0u64;
    for row in 0..FINGERPRINT_HEIGHT {
        for column in 0..FINGERPRINT_WIDTH - 1 {
            let cell = row * FINGERPRINT_WIDTH + column;
            let left = totals[cell] / counts[cell].max(1.0);
            let right = totals[cell + 1] / counts[cell + 1].max(1.0);
            hash = (hash << 1) | u64::from(left > right);
        }
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageEncoder, Rgb, RgbImage, imageops};
    use std::sync::LazyLock;

    const RESOLUTION: u32 = 512;
    const WIDTH: u32 = 384;
    const HEIGHT: u32 = 288;

    /// One large picture the fixtures are framed out of, so "another angle"
    /// means what it means with a camera: the same scene, framed elsewhere. It
    /// is 1/f noise for the ground, which is the spectrum a photograph has, with
    /// hard-edged shapes and grain on top, which is the fine detail a blur has
    /// something to destroy.
    static SCENE: LazyLock<RgbImage> = LazyLock::new(|| paint(0x5eed, 1600, 1200));

    fn frame(x: u32, y: u32) -> RgbImage {
        imageops::crop_imm(&*SCENE, x, y, WIDTH, HEIGHT).to_image()
    }

    /// Eight framings that do not overlap: one subject, eight shots.
    fn angles() -> Vec<RgbImage> {
        [
            (0, 0),
            (400, 0),
            (800, 0),
            (1200, 0),
            (0, 300),
            (400, 300),
            (800, 300),
            (1200, 300),
        ]
        .into_iter()
        .map(|(x, y)| frame(x, y))
        .collect()
    }

    fn set(images: &[RgbImage]) -> Vec<Vec<u8>> {
        images.iter().map(png).collect()
    }

    fn audit(verdict: &Verdict, count: usize) {
        let mut seen: Vec<usize> = verdict
            .keep
            .iter()
            .copied()
            .chain(verdict.drop.iter().map(|(index, _)| *index))
            .collect();
        seen.sort_unstable();
        assert_eq!(
            seen,
            (0..count).collect::<Vec<usize>>(),
            "every image must be kept or dropped exactly once, got {verdict:?}"
        );
    }

    fn lattice(seed: u64, x: i64, y: i64) -> f32 {
        let mut hash = seed
            ^ (x as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15)
            ^ (y as u64).wrapping_mul(0xc2b2_ae3d_27d4_eb4f);
        hash ^= hash >> 33;
        hash = hash.wrapping_mul(0xff51_afd7_ed55_8ccd);
        hash ^= hash >> 33;
        hash = hash.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
        hash ^= hash >> 33;
        ((hash >> 40) as f32 / 8_388_608.0) - 1.0
    }

    fn wave(seed: u64, x: f32, y: f32) -> f32 {
        let (floor_x, floor_y) = (x.floor(), y.floor());
        let (fraction_x, fraction_y) = (x - floor_x, y - floor_y);
        let ease_x = fraction_x * fraction_x * (3.0 - 2.0 * fraction_x);
        let ease_y = fraction_y * fraction_y * (3.0 - 2.0 * fraction_y);
        let (ix, iy) = (floor_x as i64, floor_y as i64);
        let top = lattice(seed, ix, iy) * (1.0 - ease_x) + lattice(seed, ix + 1, iy) * ease_x;
        let bottom =
            lattice(seed, ix, iy + 1) * (1.0 - ease_x) + lattice(seed, ix + 1, iy + 1) * ease_x;
        top * (1.0 - ease_y) + bottom * ease_y
    }

    fn octaves(seed: u64, x: f32, y: f32, count: u32) -> f32 {
        let mut value = 0.5f32;
        let mut amplitude = 0.34f32;
        let mut frequency = 3.0f32;
        for octave in 0..count {
            value += amplitude * wave(seed + u64::from(octave) * 977, x * frequency, y * frequency);
            amplitude *= 0.5;
            frequency *= 2.0;
        }
        value.clamp(0.0, 1.0) * 255.0
    }

    fn paint(seed: u64, width: u32, height: u32) -> RgbImage {
        let span = width.max(height) as f32;
        let shapes: Vec<(f32, f32, f32, f32, bool)> = (0..18u64)
            .map(|index| {
                let pick = |salt: i64| {
                    (lattice(seed ^ index.wrapping_mul(7919), salt, index as i64) + 1.0) / 2.0
                };
                (
                    pick(1) * width as f32,
                    pick(2) * height as f32,
                    0.02 * span + pick(3) * 0.12 * span,
                    pick(4) * 255.0,
                    pick(5) > 0.5,
                )
            })
            .collect();
        let mut image = RgbImage::new(width, height);
        for (x, y, pixel) in image.enumerate_pixels_mut() {
            let (u, v) = (x as f32 / span, y as f32 / span);
            let (fx, fy) = (x as f32, y as f32);
            let mut covering = None;
            for (centre_x, centre_y, radius, tone, round) in &shapes {
                let inside = if *round {
                    ((fx - centre_x).powi(2) + (fy - centre_y).powi(2)).sqrt() < *radius
                } else {
                    (fx - centre_x).abs() < *radius && (fy - centre_y).abs() < *radius * 0.7
                };
                if inside {
                    covering = Some(*tone);
                }
            }
            let value = match covering {
                Some(tone) => (tone * 3.0 + octaves(seed ^ 0xabcd, u * 4.0, v * 4.0, 9)) / 4.0,
                None => octaves(seed, u, v, 8),
            };
            *pixel = Rgb([value as u8; 3]);
        }
        image
    }

    /// A frame the camera dragged sideways during the exposure.
    fn smear(image: &RgbImage, span: u32) -> RgbImage {
        let (width, height) = image.dimensions();
        let mut out = RgbImage::new(width, height);
        for (x, y, pixel) in out.enumerate_pixels_mut() {
            let mut sum = 0u32;
            for step in 0..span {
                let sampled = (x + step).saturating_sub(span / 2).min(width - 1);
                sum += u32::from(image.get_pixel(sampled, y).0[0]);
            }
            *pixel = Rgb([(sum / span) as u8; 3]);
        }
        out
    }

    /// A subject in focus against a background thrown out of it, which is a
    /// choice a photographer makes rather than a frame to throw away.
    fn shallow(image: &RgbImage, sigma: f32) -> RgbImage {
        let (width, height) = image.dimensions();
        let mut out = imageops::blur(image, sigma);
        let (inset_x, inset_y) = (width / 5, height / 5);
        let subject = imageops::crop_imm(
            image,
            inset_x,
            inset_y,
            width - 2 * inset_x,
            height - 2 * inset_y,
        )
        .to_image();
        imageops::overlay(&mut out, &subject, i64::from(inset_x), i64::from(inset_y));
        out
    }

    fn shrink(image: &RgbImage, width: u32, height: u32) -> RgbImage {
        imageops::resize(image, width, height, imageops::FilterType::Lanczos3)
    }

    fn png(image: &RgbImage) -> Vec<u8> {
        let mut out = Vec::new();
        image::codecs::png::PngEncoder::new(&mut out)
            .write_image(
                image.as_raw(),
                image.width(),
                image.height(),
                image::ExtendedColorType::Rgb8,
            )
            .expect("encode png");
        out
    }

    fn jpeg(image: &RgbImage, quality: u8) -> Vec<u8> {
        let mut out = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality)
            .write_image(
                image.as_raw(),
                image.width(),
                image.height(),
                image::ExtendedColorType::Rgb8,
            )
            .expect("encode jpeg");
        out
    }

    #[test]
    fn an_empty_set_produces_an_empty_verdict() {
        assert_eq!(
            screen(&[], RESOLUTION),
            Verdict {
                keep: Vec::new(),
                drop: Vec::new()
            }
        );
    }

    #[test]
    fn eight_framings_of_one_subject_are_left_alone() {
        let images = set(&angles());
        let verdict = screen(&images, RESOLUTION);
        assert_eq!(
            verdict.drop,
            Vec::new(),
            "moving the camera is what a good set does"
        );
        assert_eq!(verdict.keep, (0..8).collect::<Vec<usize>>());
    }

    #[test]
    fn bytes_that_will_not_decode_are_kept() {
        let mut images = set(&angles());
        images.push(b"\x89PNG".to_vec());
        images.push(b"not an image at all".to_vec());
        let verdict = screen(&images, RESOLUTION);
        audit(&verdict, images.len());
        assert_eq!(
            verdict.drop,
            Vec::new(),
            "what screening cannot read belongs to a later stage, got {verdict:?}"
        );
    }

    #[test]
    fn a_byte_identical_frame_is_a_duplicate() {
        let mut images = set(&angles());
        images.push(images[0].clone());
        let verdict = screen(&images, RESOLUTION);
        audit(&verdict, images.len());
        assert_eq!(verdict.drop, vec![(8, Rejection::Duplicate)]);
    }

    #[test]
    fn a_re_encoded_frame_is_a_duplicate() {
        let mut images = set(&angles());
        images.push(jpeg(&frame(0, 0), 60));
        let verdict = screen(&images, RESOLUTION);
        assert_eq!(
            verdict.drop,
            vec![(8, Rejection::Duplicate)],
            "a re-encode is the same photograph, got {verdict:?}"
        );
    }

    #[test]
    fn a_frame_shifted_a_few_pixels_is_a_duplicate() {
        let mut images = set(&angles());
        images.push(png(&frame(4, 3)));
        let verdict = screen(&images, RESOLUTION);
        assert_eq!(
            verdict.drop,
            vec![(8, Rejection::Duplicate)],
            "the next frame of a burst is the same shot, got {verdict:?}"
        );
    }

    #[test]
    fn a_smeared_frame_is_blurred() {
        let mut images = set(&angles());
        images.push(png(&smear(&frame(1200, 600), 21)));
        let verdict = screen(&images, RESOLUTION);
        audit(&verdict, images.len());
        assert_eq!(verdict.drop, vec![(8, Rejection::Blurred)]);
    }

    #[test]
    fn soft_focus_is_not_motion_blur() {
        let mut images = set(&angles());
        images.push(png(&imageops::blur(&frame(400, 900), 1.0)));
        images.push(png(&shallow(&frame(0, 600), 1.0)));
        let verdict = screen(&images, RESOLUTION);
        audit(&verdict, images.len());
        assert_eq!(
            verdict.drop,
            Vec::new(),
            "a soft frame and a shallow depth of field are not a smeared exposure, got {verdict:?}"
        );
    }

    #[test]
    fn a_frame_far_under_the_training_resolution_is_dropped() {
        let mut images = set(&angles());
        images.push(png(&shrink(&frame(1200, 600), 200, 150)));
        let verdict = screen(&images, RESOLUTION);
        audit(&verdict, images.len());
        assert_eq!(verdict.drop, vec![(8, Rejection::Small)]);
    }

    #[test]
    fn a_frame_at_half_the_training_resolution_is_kept() {
        let mut images = set(&angles());
        images.push(png(&shrink(&frame(1200, 600), 341, 256)));
        let verdict = screen(&images, RESOLUTION);
        assert_eq!(
            verdict.drop,
            Vec::new(),
            "the training resize absorbs a doubling, got {verdict:?}"
        );
    }

    #[test]
    fn the_floor_holds_when_every_frame_is_a_repeat() {
        let images: Vec<Vec<u8>> = (0..8).map(|step| png(&frame(step * 2, step * 2))).collect();
        let verdict = screen(&images, RESOLUTION);
        audit(&verdict, images.len());
        assert_eq!(
            verdict.keep.len(),
            FLOOR,
            "eight copies of one pose still has to leave a workable set, got {verdict:?}"
        );
        assert!(
            verdict
                .drop
                .iter()
                .all(|(_, rejection)| *rejection == Rejection::Duplicate)
        );
    }

    #[test]
    fn the_floor_restores_repeats_before_smeared_frames() {
        let mut images: Vec<Vec<u8>> = (0..4).map(|step| png(&frame(step * 2, step * 2))).collect();
        for step in 0..4 {
            images.push(png(&smear(&frame(step * 2 + 1, step * 2 + 1), 21)));
        }
        let verdict = screen(&images, RESOLUTION);
        audit(&verdict, images.len());
        assert_eq!(verdict.keep.len(), FLOOR);
        assert!(
            verdict
                .drop
                .iter()
                .all(|(_, rejection)| *rejection == Rejection::Blurred),
            "a repeated pose costs the run less than a smeared one, got {verdict:?}"
        );
    }

    #[test]
    fn the_floor_leaves_a_short_set_alone() {
        let images: Vec<Vec<u8>> = (0..3).map(|step| png(&frame(step * 2, step * 2))).collect();
        let verdict = screen(&images, RESOLUTION);
        assert_eq!(
            verdict.keep,
            vec![0, 1, 2],
            "there is nothing to spare in a set of three, got {verdict:?}"
        );
        assert_eq!(verdict.drop, Vec::new());
    }

    #[test]
    fn a_burst_keeps_its_distinct_frames_and_drops_the_exact_copy() {
        let mut images: Vec<Vec<u8>> = (0..5).map(|step| png(&frame(step * 3, step * 3))).collect();
        images.push(images[0].clone());
        images.push(png(&smear(&frame(2, 2), 21)));
        images.push(png(&smear(&frame(5, 5), 21)));
        let verdict = screen(&images, RESOLUTION);
        audit(&verdict, images.len());
        assert_eq!(verdict.keep.len(), FLOOR);
        assert!(
            verdict.drop.contains(&(5, Rejection::Duplicate)),
            "the byte-identical twin adds nothing the burst does not already have, got {verdict:?}"
        );
        assert!(
            verdict.drop.contains(&(6, Rejection::Blurred))
                && verdict.drop.contains(&(7, Rejection::Blurred)),
            "smeared frames stay out while any repeat can take their place, got {verdict:?}"
        );
    }

    #[test]
    fn rejections_serialize_as_snake_case() {
        let names: Vec<String> = [Rejection::Duplicate, Rejection::Blurred, Rejection::Small]
            .iter()
            .map(|rejection| serde_json::to_string(rejection).expect("serialize"))
            .collect();
        assert_eq!(names, vec!["\"duplicate\"", "\"blurred\"", "\"small\""]);
    }
}
