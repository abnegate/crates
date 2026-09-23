//! End-to-end checks that training images are framed on their subject.
//!
//! Ignored by default, so `cargo test` stays free of a 168 MiB download. Run
//! them with `ABNEGATE_VISION_MODEL` naming `u2net.onnx` and `--ignored`; a
//! test fails rather than passes when the variable does not name the weights.

#![cfg(feature = "saliency")]

use abnegate_comfy::Config;
use abnegate_comfy::VISION_MODEL_VARIABLE;
use abnegate_comfy::subject::{CENTRE, Subject};
use std::path::PathBuf;

fn configured() -> Config {
    let path = std::env::var_os(VISION_MODEL_VARIABLE)
        .map(PathBuf::from)
        .filter(|path| path.is_file())
        .unwrap_or_else(|| panic!("{VISION_MODEL_VARIABLE} must name the U2-Net weights"));
    let mut config = Config::default();
    config.vision_model = Some(path);
    config
}

/// A pale field with one dark disc, so the expected subject is unambiguous.
fn scene(width: u32, height: u32, centre: (u32, u32), radius: u32) -> Vec<u8> {
    let mut pixels = vec![226u8; (width * height * 3) as usize];
    for y in 0..height {
        for x in 0..width {
            let horizontal = f64::from(x) - f64::from(centre.0);
            let vertical = f64::from(y) - f64::from(centre.1);
            if horizontal * horizontal + vertical * vertical <= f64::from(radius * radius) {
                let offset = ((y * width + x) * 3) as usize;
                pixels[offset..offset + 3].copy_from_slice(&[26, 32, 44]);
            }
        }
    }
    encode(&pixels, width, height)
}

fn encode(pixels: &[u8], width: u32, height: u32) -> Vec<u8> {
    use image::ImageEncoder;
    let mut encoded = Vec::new();
    image::codecs::png::PngEncoder::new(&mut encoded)
        .write_image(pixels, width, height, image::ExtendedColorType::Rgb8)
        .expect("encode test scene");
    encoded
}

/// The share of a rendered crop the dark subject covers.
fn coverage(pixels: &[u8]) -> f64 {
    let dark = pixels
        .as_chunks::<3>()
        .0
        .iter()
        .filter(|pixel| pixel[0] < 100 && pixel[1] < 100 && pixel[2] < 100)
        .count();
    dark as f64 / (pixels.len() / 3) as f64
}

#[ignore = "needs ABNEGATE_VISION_MODEL naming u2net.onnx"]
#[tokio::test]
async fn a_photo_is_cropped_onto_its_subject_rather_than_its_middle() {
    let config = configured();
    // A subject in the left tenth of a wide frame. The centre square of a
    // 1600x900 image spans x 350 to 1250, so a centre crop misses it entirely.
    let image = scene(1600, 900, (150, 450), 120);
    let subject = Subject::shared(&config).await;
    assert!(
        subject.available(),
        "the model at {VISION_MODEL_VARIABLE} did not load"
    );

    let framed = subject.crop(&image, 384).expect("crop");
    assert_eq!((framed.width, framed.height), (384, 384));
    assert!(
        coverage(&framed.pixels) > 0.05,
        "the subject covers only {:.1}% of the crop it is meant to be framed on",
        coverage(&framed.pixels) * 100.0
    );

    let centred = Subject::none()
        .render(
            &abnegate_vision::decode::decode(&image).unwrap(),
            384,
            CENTRE,
        )
        .expect("centre crop");
    assert!(
        coverage(&centred.pixels) < 0.001,
        "the scene is meant to be one a centre crop loses"
    );
}

#[ignore = "needs ABNEGATE_VISION_MODEL naming u2net.onnx"]
#[tokio::test]
async fn motion_decides_between_subjects_rather_than_inventing_one() {
    let config = configured();
    // Two equally salient discs. Nothing in the picture says which one is being
    // trained; in a clip, the one that moved does.
    let mut pixels = vec![226u8; (1280 * 720 * 3) as usize];
    for centre in [320u32, 960] {
        for y in 260..460u32 {
            for x in centre - 100..centre + 100 {
                let offset = ((y * 1280 + x) * 3) as usize;
                pixels[offset..offset + 3].copy_from_slice(&[26, 32, 44]);
            }
        }
    }
    let raster = abnegate_vision::decode::decode(&encode(&pixels, 1280, 720)).expect("decode");
    let subject = Subject::shared(&config).await;

    let side = 32;
    let mut moved_right = vec![0.0f32; side * side];
    let mut moved_left = vec![0.0f32; side * side];
    for row in 12..20 {
        for column in 20..28 {
            moved_right[row * side + column] = 60.0;
        }
        for column in 4..12 {
            moved_left[row * side + column] = 60.0;
        }
    }

    let neither = subject.focus(&raster, CENTRE);
    let right = subject.weighted(&raster, &moved_right, CENTRE);
    let left = subject.weighted(&raster, &moved_left, CENTRE);
    assert!(
        right.x > neither.x && left.x < neither.x,
        "motion should pick a side: left {left:?}, none {neither:?}, right {right:?}"
    );
    assert!(
        right.x < 0.8 && left.x > 0.2,
        "motion biases the model rather than replacing it: left {left:?}, right {right:?}"
    );
}

#[ignore = "needs ABNEGATE_VISION_MODEL naming u2net.onnx"]
#[test]
fn a_tripod_clip_is_framed_on_its_subject_rather_than_on_the_middle() {
    let config = configured();
    std::process::Command::new("ffmpeg")
        .arg("-version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .expect("ffmpeg must be installed to build the test clip");
    // Nothing moves but the sensor noise, which is the tripod case: the only
    // thing frame differencing can see is spread evenly over the picture, so it
    // says nothing about where the subject is. The centre 360x360 of this
    // 640x360 frame spans x 140 to 500, and the subject sits at x 117.
    let work = tempfile::tempdir().unwrap();
    let clip = work.path().join("tripod.mp4");
    let built = std::process::Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-filter_complex",
        ])
        .arg(
            "color=c=0xe2e2e2:s=640x360:r=30,noise=alls=12:allf=t+u:all_seed=7[bg];\
             color=c=0x1a2029:s=90x90:r=30,format=yuva420p,\
             geq=lum='p(X,Y)':a='if(lte(hypot(X-45,Y-45),44),255,0)'[disc];\
             [bg][disc]overlay=x=72:y=150",
        )
        .args(["-t", "2", "-pix_fmt", "yuv420p"])
        .arg(&clip)
        .status()
        .unwrap();
    assert!(built.success(), "could not build the test clip");

    let bytes = std::fs::read(&clip).unwrap();
    let options = abnegate_comfy::video::Options {
        fps: 2,
        resolution: 256,
        mirror: false,
        limit: 48,
    };
    let mut blind = config.clone();
    blind.vision_model = None;

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let detected = runtime
        .block_on(abnegate_comfy::video::extract(
            &config,
            &bytes,
            "tripod.mp4",
            options,
        ))
        .expect("extract with detection");
    let centred = runtime
        .block_on(abnegate_comfy::video::extract(
            &blind,
            &bytes,
            "tripod.mp4",
            options,
        ))
        .expect("extract without it");

    // A whole disc covers 4.6% of the crop: pi times 44 squared, scaled by the
    // 360 to 256 downscale, over 256 squared.
    assert!(
        thinnest(&centred) < 0.02,
        "the clip is meant to be one motion alone crops badly, got {:.2}%",
        thinnest(&centred) * 100.0
    );
    assert!(
        thinnest(&detected) > 0.04,
        "detection should keep the subject whole in every frame, got {:.2}%",
        thinnest(&detected) * 100.0
    );
}

/// The worst-framed frame of a clip, as the share of it the subject covers.
fn thinnest(clip: &abnegate_comfy::video::Clip) -> f64 {
    use base64::Engine;
    clip.frames
        .iter()
        .map(|frame| {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(&frame.bytes_base64)
                .unwrap();
            coverage(&abnegate_vision::decode::decode(&bytes).unwrap().pixels)
        })
        .fold(f64::MAX, f64::min)
}
