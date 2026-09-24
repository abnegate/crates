//! Writes out the training frames a clip becomes, to look at what a LoRA would
//! actually be trained on.
//!
//! ```sh
//! cargo run --example frames -p abnegate-comfy -- clip.mp4 out/
//! ```
//!
//! Build it with `--features saliency` and point `ABNEGATE_VISION_MODEL` at a
//! U2-Net model to see the crops subject detection produces.

use abnegate_comfy::{Config, video};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    use base64::Engine;
    let mut arguments = std::env::args().skip(1);
    let clip = arguments
        .next()
        .ok_or("usage: frames <clip> <output dir>")?;
    let output = arguments.next().unwrap_or_else(|| "frames".to_string());
    std::fs::create_dir_all(&output)?;

    let config = Config::from_environment();
    println!(
        "subject detection: {}",
        match &config.vision_model {
            Some(path) => path.display().to_string(),
            None => "off".to_string(),
        }
    );
    let extracted = video::extract(
        &config,
        &std::fs::read(&clip)?,
        &clip,
        video::Options::new(4, 512),
    )
    .await?;

    println!(
        "{} sampled at {:.2}/s, {} kept",
        extracted.sampled,
        extracted.sampled_fps,
        extracted.frames.len()
    );
    for frame in &extracted.frames {
        println!(
            "{} at {}ms, shot {}{}",
            frame.filename,
            frame.timestamp_milliseconds,
            frame.group,
            if frame.mirrored { ", mirrored" } else { "" }
        );
        std::fs::write(
            std::path::Path::new(&output).join(&frame.filename),
            base64::engine::general_purpose::STANDARD.decode(&frame.bytes_base64)?,
        )?;
    }
    Ok(())
}
