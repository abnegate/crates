//! Crops images onto their subject.
//!
//! ```sh
//! ABNEGATE_VISION_MODEL=models/u2net.onnx \
//!   cargo run --release -p abnegate-vision --features saliency --example autocrop -- \
//!   --size 1024 --out crops photo.jpg ...
//! ```

use std::io::Cursor;
use std::path::{Path, PathBuf};

use abnegate_vision::{Analyzer, Target};
use image::{ExtendedColorType, ImageEncoder};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut size = 1024u32;
    let mut out = PathBuf::from("crops");
    let mut inputs = Vec::new();

    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--size" => size = arguments.next().ok_or("--size needs a value")?.parse()?,
            "--out" => out = PathBuf::from(arguments.next().ok_or("--out needs a value")?),
            _ => inputs.push(PathBuf::from(argument)),
        }
    }
    if inputs.is_empty() {
        return Err("usage: autocrop [--size N] [--out DIR] IMAGE...".into());
    }

    let model = std::env::var("ABNEGATE_VISION_MODEL")
        .map_err(|_| "set ABNEGATE_VISION_MODEL to the u2net.onnx path")?;
    let analyzer = Analyzer::open(&model)?;
    std::fs::create_dir_all(&out)?;

    let target = Target::square(size);
    for input in &inputs {
        let crop = analyzer.crop(&std::fs::read(input)?, target)?;
        let destination = out.join(name(input));

        let mut encoded = Vec::new();
        image::codecs::png::PngEncoder::new(Cursor::new(&mut encoded)).write_image(
            &crop.image.pixels,
            crop.image.width,
            crop.image.height,
            ExtendedColorType::Rgb8,
        )?;
        std::fs::write(&destination, encoded)?;

        println!(
            "{:<24} {}x{}  focus ({:.4}, {:.4})  confidence {:.4}  region {}+{}+{}x{}  -> {}",
            name(input),
            crop.source.0,
            crop.source.1,
            crop.focus.point.x,
            crop.focus.point.y,
            crop.focus.confidence,
            crop.region.x,
            crop.region.y,
            crop.region.width,
            crop.region.height,
            destination.display()
        );
    }
    Ok(())
}

fn name(path: &Path) -> String {
    let stem = path
        .file_stem()
        .map_or_else(|| "crop".into(), |stem| stem.to_string_lossy());
    format!("{stem}.png")
}
