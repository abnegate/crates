//! End-to-end checks against the real U2-Net model.
//!
//! Ignored by default, so a plain `cargo test` stays free of a 168 MiB model.
//! Run them with `ABNEGATE_VISION_MODEL` pointing at `u2net.onnx`:
//! `ABNEGATE_VISION_MODEL=models/u2net.onnx cargo test -p abnegate-vision --features saliency -- --ignored`.

#![cfg(feature = "saliency")]

use std::io::Cursor;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;

use abnegate_vision::{Analyzer, Target};
use image::{ExtendedColorType, ImageEncoder};

const MODEL_VARIABLE: &str = "ABNEGATE_VISION_MODEL";

fn analyzer() -> Analyzer {
    let path = PathBuf::from(
        std::env::var_os(MODEL_VARIABLE)
            .unwrap_or_else(|| panic!("set {MODEL_VARIABLE} to the u2net.onnx path")),
    );
    Analyzer::open(path).expect("load model")
}

/// A pale field with one dark ellipse, so the expected subject is unambiguous.
fn scene(width: u32, height: u32, centre: (u32, u32), radius: (u32, u32)) -> Vec<u8> {
    let mut pixels = vec![226u8; (width * height * 3) as usize];
    for y in 0..height {
        for x in 0..width {
            let distance_x = (f64::from(x) - f64::from(centre.0)) / f64::from(radius.0);
            let distance_y = (f64::from(y) - f64::from(centre.1)) / f64::from(radius.1);
            if distance_x * distance_x + distance_y * distance_y <= 1.0 {
                let offset = ((y * width + x) * 3) as usize;
                pixels[offset..offset + 3].copy_from_slice(&[26, 32, 44]);
            }
        }
    }

    let mut encoded = Vec::new();
    image::codecs::png::PngEncoder::new(Cursor::new(&mut encoded))
        .write_image(&pixels, width, height, ExtendedColorType::Rgb8)
        .expect("encode test scene");
    encoded
}

#[test]
#[ignore = "needs ABNEGATE_VISION_MODEL pointing at u2net.onnx"]
fn crops_a_landscape_image_onto_its_subject() {
    let analyzer = analyzer();

    // Subject in the upper right of a 16:9 frame.
    let image = scene(1280, 720, (960, 216), (150, 150));
    let crop = analyzer.crop(&image, Target::square(512)).expect("crop");

    assert!(
        crop.focus.point.x > 0.55,
        "focus.x = {}",
        crop.focus.point.x
    );
    assert!(
        crop.focus.point.y < 0.45,
        "focus.y = {}",
        crop.focus.point.y
    );
    assert!(
        crop.focus.confidence > 0.5,
        "confidence = {}",
        crop.focus.confidence
    );

    // A square crop of a 1280x720 frame is 720 wide and slides over [0, 560];
    // a subject at x ~= 0.75 pushes it well to the right of centre.
    assert_eq!((crop.region.width, crop.region.height), (720, 720));
    assert!(crop.region.x > 280, "region.x = {}", crop.region.x);
    assert_eq!((crop.image.width, crop.image.height), (512, 512));
    assert_eq!(crop.image.pixels.len(), 512 * 512 * 3);
}

#[test]
#[ignore = "needs ABNEGATE_VISION_MODEL pointing at u2net.onnx"]
fn the_subject_survives_the_crop() {
    let analyzer = analyzer();

    // Subject hard against the left edge: centring on it would run off frame.
    let image = scene(1600, 900, (140, 450), (120, 200));
    let crop = analyzer.crop(&image, Target::square(384)).expect("crop");

    assert_eq!(
        crop.region.x, 0,
        "the crop should sit flush against the edge"
    );

    // The dark subject has to actually be inside the rendered output.
    let dark = crop
        .image
        .pixels
        .as_chunks::<3>()
        .0
        .iter()
        .filter(|pixel| pixel[0] < 100 && pixel[1] < 100 && pixel[2] < 100)
        .count();
    let coverage = dark as f64 / (384.0 * 384.0);
    assert!(
        coverage > 0.05,
        "subject covers only {:.1}% of the crop",
        coverage * 100.0
    );
}

#[test]
#[ignore = "needs ABNEGATE_VISION_MODEL pointing at u2net.onnx"]
fn a_caller_can_weigh_the_map_before_it_is_reduced_to_a_point() {
    let analyzer = analyzer();

    // Two equally salient subjects, one left and one right.
    let mut pixels = vec![226u8; (1280 * 720 * 3) as usize];
    for centre in [320u32, 960] {
        for y in 210..510u32 {
            for x in centre - 150..centre + 150 {
                let offset = ((y * 1280 + x) * 3) as usize;
                pixels[offset..offset + 3].copy_from_slice(&[26, 32, 44]);
            }
        }
    }
    let mut image = Vec::new();
    image::codecs::png::PngEncoder::new(Cursor::new(&mut image))
        .write_image(&pixels, 1280, 720, ExtendedColorType::Rgb8)
        .expect("encode test scene");
    let raster = abnegate_vision::decode::decode(&image).expect("decode");

    let unweighted = analyzer
        .saliency(&raster, |map, content| {
            abnegate_vision::gravity::from_saliency_region(map, 320, 320, content)
                .expect("centroid")
        })
        .expect("saliency");
    assert!(
        (unweighted.0.x - 0.5).abs() < 0.1,
        "two matching subjects should average out near the middle, got {:?}",
        unweighted.0
    );

    // Weighting the right half is what a caller who knows which subject is
    // theirs would do, and it has to move the answer.
    let weighted = analyzer
        .saliency(&raster, |map, content| {
            let biased: Vec<f32> = map
                .iter()
                .enumerate()
                .map(|(index, value)| {
                    let x = (index % 320) as i32;
                    if x > (content.min_x + content.max_x) / 2 {
                        value * 2.0
                    } else {
                        *value
                    }
                })
                .collect();
            abnegate_vision::gravity::from_saliency_region(&biased, 320, 320, content)
                .expect("centroid")
        })
        .expect("saliency");
    // Two equal masses a quarter-frame apart, one weighted twice: the centre of
    // mass moves a twelfth of the frame towards it.
    assert!(
        (weighted.0.x - (unweighted.0.x + 1.0 / 12.0)).abs() < 0.02,
        "weighting the right subject should pull the focus right: {:?} -> {:?}",
        unweighted.0,
        weighted.0
    );
}

#[test]
#[ignore = "needs ABNEGATE_VISION_MODEL pointing at u2net.onnx"]
fn a_panic_in_the_callers_reader_leaves_the_analyzer_usable() {
    let analyzer = analyzer();
    let raster =
        abnegate_vision::decode::decode(&scene(640, 480, (320, 240), (100, 100))).expect("decode");

    let panicked = catch_unwind(AssertUnwindSafe(|| {
        analyzer.saliency(&raster, |_, _| -> () {
            panic!("the caller's reader failed while the model was locked")
        })
    }));
    assert!(panicked.is_err());

    analyzer
        .focus_raster(&raster)
        .expect("the next inference still runs");
}
