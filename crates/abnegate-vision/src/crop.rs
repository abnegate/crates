//! Subject-aware crop planning and rendering.

use fast_image_resize::images::{Image, ImageRef};
use fast_image_resize::{FilterType, PixelType, ResizeAlg, ResizeOptions, Resizer};

use crate::decode::{Layout, MAXIMUM_PIXELS, Orientation, Raster};
use crate::gravity::Point;
use crate::preprocess::map_pixel;

mod error;
mod region;
mod rendered;
mod target;

pub use crate::crop::error::CropError;
pub use crate::crop::region::Region;
pub use crate::crop::rendered::Rendered;
pub use crate::crop::target::Target;

/// Picks the largest region of the target's aspect ratio that fits inside the
/// image, then slides it so its centre sits as close to `focus` as the bounds
/// allow.
///
/// Sliding rather than clamping the centre is what keeps a subject near an edge
/// fully in frame: a face 4 % from the top stays whole instead of being cut in
/// half by a crop centred on it.
///
/// `focus` is normalized to the oriented image, which is the space
/// `Analyzer` reports focal points in.
pub fn plan(size: (u32, u32), target: Target, focus: Point) -> Result<Region, CropError> {
    let (width, height) = size;
    if width == 0 || height == 0 {
        return Err(CropError::EmptySource);
    }
    if target.width == 0 || target.height == 0 {
        return Err(CropError::EmptyTarget);
    }

    let aspect = target.aspect();
    let source_aspect = f64::from(width) / f64::from(height);
    let (crop_width, crop_height) = if source_aspect > aspect {
        (scaled(f64::from(height) * aspect, width), height)
    } else {
        (width, scaled(f64::from(width) / aspect, height))
    };

    Ok(Region::new(
        offset(focus.x, crop_width, width),
        offset(focus.y, crop_height, height),
        crop_width,
        crop_height,
    ))
}

/// Renders a planned region to the target size.
///
/// The crop, the downscale, and any EXIF rotation happen in one pass over the
/// source: `fast_image_resize` samples the region directly, so no intermediate
/// full-size copy is made, and the orientation is applied to the already-small
/// output.
///
/// `region` is in the oriented image and has to lie wholly inside it, and the
/// target is held to the same [`MAXIMUM_PIXELS`] ceiling as a decoded image.
pub fn render(raster: &Raster, region: Region, target: Target) -> Result<Rendered, CropError> {
    if raster.width == 0 || raster.height == 0 {
        return Err(CropError::EmptySource);
    }
    if target.width == 0 || target.height == 0 {
        return Err(CropError::EmptyTarget);
    }
    if target.area() > MAXIMUM_PIXELS {
        return Err(CropError::TargetTooLarge {
            width: target.width,
            height: target.height,
        });
    }
    let size = raster.oriented_size();
    if !region.lies_within(size) {
        return Err(CropError::Region { region, size });
    }

    let source = to_source(raster.orientation, region, (raster.width, raster.height));
    let (resized_width, resized_height) = if raster.orientation.swaps_axes() {
        (target.height, target.width)
    } else {
        (target.width, target.height)
    };

    let pixel_type = match raster.layout {
        Layout::Rgb => PixelType::U8x3,
        Layout::Rgba => PixelType::U8x4,
    };
    let channels = raster.layout.channels();
    let view = ImageRef::new(raster.width, raster.height, &raster.pixels, pixel_type)?;
    let mut resized = Image::new(resized_width, resized_height, pixel_type);

    let options = ResizeOptions::new()
        .resize_alg(ResizeAlg::Convolution(FilterType::Lanczos3))
        .use_alpha(false)
        .crop(
            f64::from(source.x),
            f64::from(source.y),
            f64::from(source.width),
            f64::from(source.height),
        );
    Resizer::new().resize(&view, &mut resized, &options)?;

    Ok(Rendered::new(
        target.width,
        target.height,
        orient(
            resized.buffer(),
            (resized_width, resized_height),
            target,
            raster.orientation,
            channels,
        ),
    ))
}

/// Maps a region of the oriented image back to the stored pixel buffer.
fn to_source(orientation: Orientation, region: Region, size: (u32, u32)) -> Region {
    let near = map_pixel(orientation, region.x, region.y, size);
    let far = map_pixel(
        orientation,
        region.x + region.width - 1,
        region.y + region.height - 1,
        size,
    );
    Region::new(
        near.0.min(far.0),
        near.1.min(far.1),
        near.0.abs_diff(far.0) + 1,
        near.1.abs_diff(far.1) + 1,
    )
}

/// Applies EXIF orientation to the rendered crop and flattens it to RGB.
fn orient(
    resized: &[u8],
    size: (u32, u32),
    target: Target,
    orientation: Orientation,
    channels: usize,
) -> Vec<u8> {
    let stride = size.0 as usize * channels;
    let mut pixels = vec![0u8; target.width as usize * target.height as usize * 3];

    for y in 0..target.height {
        for x in 0..target.width {
            let (source_x, source_y) = map_pixel(orientation, x, y, size);
            let source = source_y as usize * stride + source_x as usize * channels;
            let destination = (y as usize * target.width as usize + x as usize) * 3;
            pixels[destination..destination + 3].copy_from_slice(&resized[source..source + 3]);
        }
    }
    pixels
}

fn scaled(value: f64, limit: u32) -> u32 {
    (value.round() as i64).clamp(1, i64::from(limit)) as u32
}

/// Centres a crop of `span` on `focus`, then slides it inside `[0, limit]`.
fn offset(focus: f64, span: u32, limit: u32) -> u32 {
    let centre = focus.clamp(0.0, 1.0) * f64::from(limit);
    let start = centre - f64::from(span) / 2.0;
    (start.round() as i64).clamp(0, i64::from(limit.saturating_sub(span))) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    const CENTRE: Point = Point { x: 0.5, y: 0.5 };

    fn raster(width: u32, height: u32, orientation: Orientation) -> Raster {
        let mut pixels = Vec::with_capacity((width * height) as usize * 3);
        for y in 0..height {
            for x in 0..width {
                pixels.extend_from_slice(&[x as u8, y as u8, 0]);
            }
        }
        Raster::new(width, height, Layout::Rgb, pixels).with_orientation(orientation)
    }

    #[test]
    fn square_crop_of_a_landscape_image_takes_the_full_height() {
        let region = plan((1600, 900), Target::square(512), CENTRE).unwrap();
        assert_eq!(region, Region::new(350, 0, 900, 900));
    }

    #[test]
    fn square_crop_of_a_portrait_image_takes_the_full_width() {
        let region = plan((900, 1600), Target::square(512), CENTRE).unwrap();
        assert_eq!(region, Region::new(0, 350, 900, 900));
    }

    #[test]
    fn the_crop_follows_the_subject() {
        // A 900px square in a 1600px-wide image can slide over [0, 700].
        for (focus, want) in [(0.25, 0), (0.5, 350), (0.6, 510), (0.8, 700)] {
            let region = plan((1600, 900), Target::square(1), Point { x: focus, y: 0.5 }).unwrap();
            assert_eq!(region.x, want, "focus {focus}");
        }
    }

    #[test]
    fn a_subject_near_an_edge_stays_whole() {
        // Centring on the subject would put the crop at -410; sliding keeps the
        // full 900px square in frame instead of cutting the subject in half.
        let region = plan((1600, 900), Target::square(512), Point { x: 0.025, y: 0.5 }).unwrap();
        assert_eq!(region.x, 0);
        assert_eq!(region.width, 900);

        let region = plan((1600, 900), Target::square(512), Point { x: 0.99, y: 0.5 }).unwrap();
        assert_eq!(region.x, 1600 - 900);
    }

    #[test]
    fn a_wide_target_crops_vertically() {
        let region = plan((1000, 1000), Target::new(16, 9), Point { x: 0.5, y: 0.2 }).unwrap();
        assert_eq!(region.width, 1000);
        assert_eq!(region.height, 563);
        assert_eq!(region.y, 0);
    }

    #[test]
    fn the_planned_region_always_fits_inside_the_image() {
        for focus in [0.0, 0.1, 0.5, 0.9, 1.0] {
            for size in [(37, 91), (91, 37), (64, 64), (1, 500)] {
                let region = plan(size, Target::square(256), Point { x: focus, y: focus }).unwrap();
                assert!(region.x + region.width <= size.0, "{region:?} in {size:?}");
                assert!(region.y + region.height <= size.1, "{region:?} in {size:?}");
                assert!(region.width >= 1 && region.height >= 1);
            }
        }
    }

    #[test]
    fn rejects_degenerate_sizes() {
        assert!(matches!(
            plan((0, 10), Target::square(8), CENTRE),
            Err(CropError::EmptySource)
        ));
        assert!(matches!(
            plan((10, 10), Target::new(0, 8), CENTRE),
            Err(CropError::EmptyTarget)
        ));
    }

    #[test]
    fn renders_the_requested_size() {
        let source = raster(80, 60, Orientation::Normal);
        let region = plan((80, 60), Target::square(32), CENTRE).unwrap();
        let rendered = render(&source, region, Target::square(32)).unwrap();

        assert_eq!((rendered.width, rendered.height), (32, 32));
        assert_eq!(rendered.pixels.len(), 32 * 32 * 3);
    }

    #[test]
    fn rendering_an_oriented_image_matches_rendering_the_rotated_pixels() {
        // A quarter-turn of the source, cropped identically, must produce the
        // same output as the already-upright image.
        let upright = raster(64, 48, Orientation::Normal);
        // Rotate90 maps display (x, y) to source (y, 63 - x), so the stored
        // buffer holds upright(63 - row, column).
        let mut rotated = Vec::with_capacity(upright.pixels.len());
        for row in 0..64u32 {
            for column in 0..48u32 {
                let source = (column as usize * 64 + (63 - row) as usize) * 3;
                rotated.extend_from_slice(&upright.pixels[source..source + 3]);
            }
        }
        let sideways =
            Raster::new(48, 64, Layout::Rgb, rotated).with_orientation(Orientation::Rotate90);
        assert_eq!(sideways.oriented_size(), (64, 48));

        let region = plan((64, 48), Target::square(16), CENTRE).unwrap();
        let expected = render(&upright, region, Target::square(16)).unwrap();
        let actual = render(&sideways, region, Target::square(16)).unwrap();

        // The convolution runs its two passes in the opposite order here, so the
        // fixed-point rounding can differ by a step; the geometry cannot.
        assert_eq!(actual.pixels.len(), expected.pixels.len());
        for (index, (got, want)) in actual.pixels.iter().zip(&expected.pixels).enumerate() {
            assert!(
                got.abs_diff(*want) <= 2,
                "byte {index}: got {got}, want {want}"
            );
        }
    }

    #[test]
    fn a_region_outside_the_image_is_refused_rather_than_rendered() {
        let source = raster(80, 60, Orientation::Normal);
        for region in [
            Region::new(0, 0, 0, 10),
            Region::new(u32::MAX, 0, 2, 2),
            Region::new(70, 0, 20, 20),
        ] {
            let error = render(&source, region, Target::square(8)).expect_err("out of bounds");
            assert!(
                matches!(error, CropError::Region { size: (80, 60), .. }),
                "{region:?} gave {error:?}"
            );
        }
    }

    #[test]
    fn a_region_is_checked_against_the_oriented_image() {
        let sideways = raster(48, 64, Orientation::Rotate90);
        let upright_whole = Region::new(0, 0, 64, 48);
        assert!(render(&sideways, upright_whole, Target::square(8)).is_ok());

        let stored_whole = Region::new(0, 0, 48, 64);
        assert!(matches!(
            render(&sideways, stored_whole, Target::square(8)),
            Err(CropError::Region { size: (64, 48), .. })
        ));
    }

    #[test]
    fn a_target_beyond_the_pixel_ceiling_is_refused_before_it_is_allocated() {
        let source = raster(8, 8, Orientation::Normal);
        let whole = Region::new(0, 0, 8, 8);
        assert!(matches!(
            render(&source, whole, Target::new(5_000, 4_001)),
            Err(CropError::TargetTooLarge {
                width: 5_000,
                height: 4_001
            })
        ));
    }
}
