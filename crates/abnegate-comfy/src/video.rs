//! Turns a submitted clip into a LoRA training set.
//!
//! A video is a worse photo set than it looks. Neighbouring frames repeat each
//! other, and whichever frame the clock lands on is as likely to be smeared by
//! motion as it is to be sharp. Sampling therefore runs above the rate the
//! caller asked for and then earns that rate back: the sharpest frame of each
//! moment wins its slot, and a frame that repeats a shot already kept is
//! dropped rather than counted twice.
//!
//! What survives is cropped on its subject and mirrored in alternation, so a
//! subject filmed from one side does not teach the adapter that it only ever
//! faces that way. Finding the subject is where a clip has an advantage over a
//! photo: motion says which of the things in frame is being filmed, and it
//! costs nothing here because the same frame differences already decide which
//! frames are worth keeping.

use crate::config::Config;
use crate::lora::{TrainError, png};
use crate::subject::Subject;
use abnegate_vision::crop::{self, Region, Rendered, Target};
use abnegate_vision::gravity::{self, Point};
use abnegate_vision::{Raster, decode};
use base64::Engine;
use serde::{Deserialize, Serialize};
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Output;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;

/// Sampling runs at this multiple of the requested rate so every kept frame is
/// the sharpest of the ones that competed for its slot.
const OVERSAMPLE: f64 = 2.0;
/// Longest edge ffmpeg writes. Decoding 4K costs time the crop throws away.
const WORKING_EDGE: u32 = 1920;
/// Frames one clip may sample. A long clip lowers its rate to stay under this
/// rather than truncating, so the whole video is still represented.
const CEILING: usize = 900;
/// Side of the square every frame is measured on. Small enough to be cheap,
/// large enough that a motion-blurred frame still scores below a sharp one.
const ANALYSIS: u32 = 256;
/// Side of the motion grid the subject is located on.
const GRID: usize = 32;
/// Motion below this share of the frame's peak is background, not the subject.
const MOTION_FLOOR: f32 = 0.55;
/// Sampled frames each side of a frame whose motion counts towards its crop.
/// One is enough to steady a jittery reading without dragging the crop back
/// towards where the subject used to be.
const MOTION_WINDOW: usize = 1;
/// Mean motion per cell under which a frame is treated as still.
const STILL: f64 = 1.5;
/// Differing bits under which two frames are the same shot.
const DUPLICATE_DISTANCE: u32 = 6;
/// Differing bits under which two frames can share one caption.
const GROUP_DISTANCE: u32 = 14;
/// Frames kept before near-duplicate rejection is allowed to stop early. Below
/// this a static clip would train on a single pose.
const FLOOR: usize = 8;
/// How long ffprobe may take to read a clip's duration.
const PROBE_TIMEOUT: Duration = Duration::from_secs(30);
/// How long ffmpeg may take to sample a clip at most [`CEILING`] frames.
const DECODE_TIMEOUT: Duration = Duration::from_secs(600);
/// The only protocol ffmpeg and ffprobe may open, so a playlist or reference
/// inside a clip cannot reach the network or another file scheme.
const INPUT_PROTOCOLS: &str = "file";

/// A container a clip may be read as: the extension it is written under and
/// the demuxer forced on it, so ffmpeg never picks one by probing the upload.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Container {
    extension: &'static str,
    demuxer: &'static str,
}

const MP4: Container = Container {
    extension: "mp4",
    demuxer: "mov",
};

/// Every container a clip is read as. `m4v` is the MPEG-4 file, not the raw
/// elementary stream ffmpeg's own `m4v` demuxer reads.
const CONTAINERS: [Container; 6] = [
    MP4,
    Container {
        extension: "mov",
        demuxer: "mov",
    },
    Container {
        extension: "m4v",
        demuxer: "mov",
    },
    Container {
        extension: "webm",
        demuxer: "matroska",
    },
    Container {
        extension: "mkv",
        demuxer: "matroska",
    },
    Container {
        extension: "avi",
        demuxer: "avi",
    },
];

/// How a clip is turned into training images.
#[derive(Clone, Copy, Debug)]
pub struct Options {
    /// Frames kept per second of video.
    pub fps: u32,
    /// Side of the square crop every frame is rendered at.
    pub resolution: u32,
    /// Mirror alternate frames within each second.
    pub mirror: bool,
    /// Frames kept in total. The packaged trainer caps its step count, so past
    /// this each extra frame is seen fewer times without adding variety the
    /// selection has not already taken.
    pub limit: usize,
}

/// A clip submitted for training.
#[derive(Debug, Deserialize)]
pub struct FrameRequest {
    pub filename: String,
    pub bytes_base64: String,
    /// Frames kept per second. Falls back to the configured rate.
    #[serde(default)]
    pub fps: Option<u32>,
    /// Mirror alternate frames. Worth turning off for a subject carrying text
    /// or anything else a mirror would render backwards.
    #[serde(default)]
    pub mirror: Option<bool>,
}

/// One training image pulled from a clip.
#[derive(Clone, Debug, Serialize)]
pub struct Frame {
    pub filename: String,
    pub bytes_base64: String,
    #[serde(rename = "timestamp_ms")]
    pub timestamp_milliseconds: u64,
    pub mirrored: bool,
    /// Frames sharing a group are the same shot, so one caption describes them
    /// all and the vision model only has to look at one of them.
    pub group: usize,
}

/// What one clip yielded.
#[derive(Clone, Debug, Serialize)]
pub struct Clip {
    pub frames: Vec<Frame>,
    /// Frames pulled out of the video before selection.
    pub sampled: usize,
    /// Rate those frames were pulled at, which drops below the requested rate
    /// only when the clip is long enough to hit the sampling ceiling.
    pub sampled_fps: f64,
}

/// Everything one sampled frame is judged on. The pixels are not kept: only the
/// frames that survive selection are decoded a second time to be cropped.
struct Measured {
    path: PathBuf,
    timestamp_milliseconds: u64,
    sharpness: f64,
    brightness: f64,
    contrast: f64,
    luma: Vec<f32>,
    hash: u64,
}

pub async fn extract(
    config: &Config,
    video: &[u8],
    filename: &str,
    options: Options,
) -> Result<Clip, TrainError> {
    if video.is_empty() {
        return Err(TrainError::Invalid("video is empty"));
    }
    let work = tempfile::tempdir().map_err(|error| TrainError::Failed(error.to_string()))?;
    let container = container(filename);
    let clip = work.path().join(format!("clip.{}", container.extension));
    std::fs::write(&clip, video).map_err(|error| TrainError::Failed(error.to_string()))?;

    let requested = f64::from(options.fps.clamp(1, 30));
    let sampled_fps = match duration(config, container, &clip, PROBE_TIMEOUT).await {
        Some(seconds) if seconds > 0.0 => {
            (CEILING as f64 / seconds).clamp(0.5, requested * OVERSAMPLE)
        }
        _ => requested * OVERSAMPLE,
    };
    let stills = work.path().join("frames");
    std::fs::create_dir_all(&stills).map_err(|error| TrainError::Failed(error.to_string()))?;
    sample(
        config,
        container,
        &clip,
        &stills,
        sampled_fps,
        DECODE_TIMEOUT,
    )
    .await?;

    // Decoding, measuring, cropping and encoding hundreds of frames is seconds
    // of CPU that would otherwise sit on a runtime thread other requests need.
    let subject = Subject::shared(config);
    tokio::task::spawn_blocking(move || build(&stills, sampled_fps, options, &subject))
        .await
        .map_err(|error| TrainError::Failed(error.to_string()))?
}

/// Reads the sampled stills back, picks the frames worth training on, and
/// renders each one. Blocking from end to end.
fn build(
    stills: &Path,
    sampled_fps: f64,
    options: Options,
    subject: &Subject,
) -> Result<Clip, TrainError> {
    let measured = measure(stills, sampled_fps)?;
    if measured.is_empty() {
        return Err(TrainError::Invalid("video has no readable frames"));
    }
    let motion = motion(&measured);
    let chosen = choose(&measured, options.fps.max(1) as usize, options.limit);
    let groups = group(&chosen, &measured);

    let mut frames = Vec::with_capacity(chosen.len());
    let mut second = u64::MAX;
    let mut within = 0usize;
    for (position, &index) in chosen.iter().enumerate() {
        let frame = &measured[index];
        if frame.timestamp_milliseconds / 1000 == second {
            within += 1;
        } else {
            second = frame.timestamp_milliseconds / 1000;
            within = 0;
        }
        let mirrored = options.mirror && within % 2 == 1;
        let bytes = render(subject, frame, &motion[index], options.resolution, mirrored)?;
        frames.push(Frame {
            filename: format!("frame-{position:04}.png"),
            bytes_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
            timestamp_milliseconds: frame.timestamp_milliseconds,
            mirrored,
            group: groups[position],
        });
    }
    Ok(Clip {
        frames,
        sampled: measured.len(),
        sampled_fps,
    })
}

/// Seconds of video, as ffprobe reports them.
async fn duration(
    config: &Config,
    container: Container,
    clip: &Path,
    budget: Duration,
) -> Option<f64> {
    let output = execute(&config.ffprobe, &probe_arguments(container, clip), budget)
        .await
        .ok()??;
    String::from_utf8_lossy(&output.stdout).trim().parse().ok()
}

async fn sample(
    config: &Config,
    container: Container,
    clip: &Path,
    stills: &Path,
    fps: f64,
    budget: Duration,
) -> Result<(), TrainError> {
    let arguments = sample_arguments(container, clip, stills, fps);
    let output = execute(&config.ffmpeg, &arguments, budget)
        .await
        .map_err(|error| match error.kind() {
            io::ErrorKind::NotFound => TrainError::Disabled,
            _ => TrainError::Failed(error.to_string()),
        })?
        .ok_or(TrainError::Invalid(
            "the video took too long to decode; try a shorter clip",
        ))?;
    if !output.status.success() {
        tracing::warn!(
            reason = %crate::excerpt::tail(&String::from_utf8_lossy(&output.stderr)),
            "ffmpeg could not read a submitted clip"
        );
        return Err(TrainError::Invalid(
            "the video could not be decoded; try re-exporting it as H.264 MP4",
        ));
    }
    Ok(())
}

/// Runs a decoder to completion, or kills it once `budget` runs out and
/// answers `None`.
async fn execute(
    program: &str,
    arguments: &[OsString],
    budget: Duration,
) -> io::Result<Option<Output>> {
    let mut command = Command::new(program);
    command
        .args(arguments)
        .stdin(Stdio::null())
        .kill_on_drop(true);
    match tokio::time::timeout(budget, command.output()).await {
        Ok(output) => output.map(Some),
        Err(_) => Ok(None),
    }
}

/// Options that pin how `clip` is opened. They have to precede `-i`.
fn input_arguments(container: Container, clip: &Path) -> Vec<OsString> {
    let mut arguments: Vec<OsString> = [
        "-protocol_whitelist",
        INPUT_PROTOCOLS,
        "-f",
        container.demuxer,
        "-i",
    ]
    .into_iter()
    .map(OsString::from)
    .collect();
    arguments.push(clip.as_os_str().to_os_string());
    arguments
}

fn probe_arguments(container: Container, clip: &Path) -> Vec<OsString> {
    let mut arguments: Vec<OsString> = [
        "-v",
        "error",
        "-select_streams",
        "v:0",
        "-show_entries",
        "format=duration",
        "-of",
        "default=noprint_wrappers=1:nokey=1",
    ]
    .into_iter()
    .map(OsString::from)
    .collect();
    arguments.extend(input_arguments(container, clip));
    arguments
}

fn sample_arguments(container: Container, clip: &Path, stills: &Path, fps: f64) -> Vec<OsString> {
    let filter = format!(
        "fps={fps:.4},scale='min({WORKING_EDGE},iw)':'min({WORKING_EDGE},ih)':\
         force_original_aspect_ratio=decrease:force_divisible_by=2"
    );
    let mut arguments: Vec<OsString> = ["-hide_banner", "-nostdin", "-loglevel", "error", "-y"]
        .into_iter()
        .map(OsString::from)
        .collect();
    arguments.extend(input_arguments(container, clip));
    arguments.extend(
        [
            "-map",
            "0:v:0",
            "-vf",
            &filter,
            "-frames:v",
            &CEILING.to_string(),
            "-q:v",
            "2",
            "-f",
            "image2",
        ]
        .into_iter()
        .map(OsString::from),
    );
    arguments.push(stills.join("%06d.jpg").into_os_string());
    arguments
}

fn measure(stills: &Path, fps: f64) -> Result<Vec<Measured>, TrainError> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(stills)
        .map_err(|error| TrainError::Failed(error.to_string()))?
        .filter_map(|entry| entry.ok().map(|item| item.path()))
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("jpg"))
        .collect();
    paths.sort();
    let mut measured = Vec::with_capacity(paths.len());
    for (index, path) in paths.into_iter().enumerate() {
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let Ok(raster) = decode::decode(&bytes) else {
            continue;
        };
        let Ok(analysis) = square(&raster, ANALYSIS) else {
            continue;
        };
        let Ok(thumbnail) = frame(&raster, Target::new(9, 8)) else {
            continue;
        };
        let plane = luma(&analysis.pixels);
        let (brightness, contrast) = spread(&plane);
        measured.push(Measured {
            path,
            timestamp_milliseconds: (index as f64 * 1000.0 / fps).round() as u64,
            sharpness: sharpness(&plane, ANALYSIS as usize),
            brightness,
            contrast,
            hash: hash(&luma(&thumbnail.pixels)),
            luma: shrink(&plane, ANALYSIS as usize, GRID),
        });
    }
    Ok(measured)
}

/// Motion energy per frame, as a grid of where the subject is. In a handheld
/// clip everything moves a little and the subject moves most, which is enough
/// to frame the crop on it.
///
/// A cell counts only when it differs from the frame before *and* the frame
/// after. Differencing against one neighbour lights up both where the subject
/// was and where it now is, and a crop centred between the two trails the
/// subject by however far it travelled.
fn motion(measured: &[Measured]) -> Vec<Vec<f32>> {
    let cells = GRID * GRID;
    let mut deltas = vec![vec![0.0f32; cells]; measured.len()];
    for index in 1..measured.len().saturating_sub(1) {
        let previous = &measured[index - 1].luma;
        let current = &measured[index].luma;
        let next = &measured[index + 1].luma;
        for cell in 0..cells {
            deltas[index][cell] = (current[cell] - previous[cell])
                .abs()
                .min((current[cell] - next[cell]).abs());
        }
    }
    if measured.len() > 2 {
        deltas[0] = deltas[1].clone();
        let last = measured.len() - 1;
        deltas[last] = deltas[last - 1].clone();
    }
    (0..measured.len())
        .map(|index| {
            let first = index.saturating_sub(MOTION_WINDOW);
            let last = (index + MOTION_WINDOW).min(measured.len() - 1);
            let mut window = vec![0.0f32; cells];
            for delta in &deltas[first..=last] {
                for cell in 0..cells {
                    window[cell] += delta[cell];
                }
            }
            window
        })
        .collect()
}

/// The frames worth training on, in time order.
fn choose(measured: &[Measured], per_second: usize, limit: usize) -> Vec<usize> {
    let lit: Vec<usize> = (0..measured.len())
        .filter(|&index| {
            let frame = &measured[index];
            frame.brightness > 6.0 && frame.brightness < 249.0 && frame.contrast > 2.0
        })
        .collect();
    let usable = if lit.is_empty() {
        (0..measured.len()).collect()
    } else {
        lit
    };

    let mut sharpest: Vec<usize> = Vec::new();
    let mut bucket: Vec<usize> = Vec::new();
    let mut second = u64::MAX;
    for &index in &usable {
        let at = measured[index].timestamp_milliseconds / 1000;
        if at != second && !bucket.is_empty() {
            sharpest.extend(pick(&bucket, measured, per_second));
            bucket.clear();
        }
        second = at;
        bucket.push(index);
    }
    sharpest.extend(pick(&bucket, measured, per_second));
    sharpest.sort_unstable();

    let mut chosen = diversify(&sharpest, measured, limit.max(1));
    chosen.sort_unstable_by_key(|&index| measured[index].timestamp_milliseconds);
    chosen
}

/// The `count` sharpest of one second's frames.
fn pick(bucket: &[usize], measured: &[Measured], count: usize) -> Vec<usize> {
    let mut ranked = bucket.to_vec();
    ranked.sort_by(|&left, &right| {
        measured[right]
            .sharpness
            .total_cmp(&measured[left].sharpness)
    });
    ranked.truncate(count);
    ranked
}

/// Farthest-point selection over the frame hashes: start from the sharpest
/// frame and keep adding whichever frame is least like everything kept so far.
/// It fills the budget with the widest spread of shots the clip contains, and
/// stops early once the only frames left repeat one already taken.
fn diversify(candidates: &[usize], measured: &[Measured], budget: usize) -> Vec<usize> {
    if candidates.len() <= 1 {
        return candidates.to_vec();
    }
    let Some((sharpest, _)) =
        candidates
            .iter()
            .copied()
            .enumerate()
            .max_by(|&(_, left), &(_, right)| {
                measured[left]
                    .sharpness
                    .total_cmp(&measured[right].sharpness)
            })
    else {
        return Vec::new();
    };
    let mut taken = vec![false; candidates.len()];
    let mut distances = vec![u32::MAX; candidates.len()];
    // Sized as it grows rather than reserved: the budget is small, and a
    // capacity taken from the clip is a promise about the clip's size.
    let mut chosen = Vec::new();
    let mut next = sharpest;
    while chosen.len() < budget.min(candidates.len()) {
        taken[next] = true;
        chosen.push(candidates[next]);
        let added = measured[candidates[next]].hash;
        for (slot, &index) in candidates.iter().enumerate() {
            distances[slot] = distances[slot].min((measured[index].hash ^ added).count_ones());
        }
        let Some((furthest, distance)) = distances
            .iter()
            .copied()
            .enumerate()
            .filter(|(slot, _)| !taken[*slot])
            .max_by_key(|&(_, distance)| distance)
        else {
            break;
        };
        if distance < DUPLICATE_DISTANCE && chosen.len() >= FLOOR.min(candidates.len()) {
            break;
        }
        next = furthest;
    }
    chosen
}

/// Groups frames that show the same shot, so captioning describes each shot
/// once instead of paying a vision-model round trip per near-identical frame.
fn group(chosen: &[usize], measured: &[Measured]) -> Vec<usize> {
    let mut representatives: Vec<u64> = Vec::new();
    chosen
        .iter()
        .map(|&index| {
            let hash = measured[index].hash;
            match representatives
                .iter()
                .position(|&against| (hash ^ against).count_ones() < GROUP_DISTANCE)
            {
                Some(group) => group,
                None => {
                    representatives.push(hash);
                    representatives.len() - 1
                }
            }
        })
        .collect()
}

/// Decodes a chosen frame again and renders the square crop that trains on it.
///
/// Subject detection leads and motion biases it, so in a crowd the crop lands
/// on the person being filmed rather than on whoever the model liked most.
/// Without the model, motion decides on its own.
fn render(
    subject: &Subject,
    measured: &Measured,
    motion: &[f32],
    resolution: u32,
    mirrored: bool,
) -> Result<Vec<u8>, TrainError> {
    let bytes =
        std::fs::read(&measured.path).map_err(|error| TrainError::Failed(error.to_string()))?;
    let raster = decode::decode(&bytes).map_err(|error| TrainError::Failed(error.to_string()))?;
    let mut rendered = subject
        .render(
            &raster,
            resolution,
            subject.weighted(&raster, motion, focus(motion)),
        )
        .map_err(|error| TrainError::Failed(error.to_string()))?;
    if mirrored {
        mirror(&mut rendered);
    }
    png(&rendered)
}

/// Where the subject is, taken as the centre of mass of what moved. Cells below
/// a share of the frame's peak are zeroed first: without that the still
/// background outweighs the subject and every crop drifts back to the middle.
fn focus(motion: &[f32]) -> Point {
    let peak = motion.iter().copied().fold(0.0f32, f32::max);
    let mean = f64::from(motion.iter().sum::<f32>()) / motion.len() as f64;
    if peak <= 0.0 || mean < STILL {
        return Point { x: 0.5, y: 0.5 };
    }
    let floor = peak * MOTION_FLOOR;
    let subject: Vec<f32> = motion
        .iter()
        .map(|&value| if value >= floor { value } else { 0.0 })
        .collect();
    gravity::from_saliency(&subject, GRID as i32, GRID as i32)
        .map(|(point, _)| point)
        .unwrap_or(Point { x: 0.5, y: 0.5 })
}

fn mirror(rendered: &mut Rendered) {
    let stride = rendered.width as usize * 3;
    for row in rendered.pixels.chunks_exact_mut(stride) {
        let mut left = 0usize;
        let mut right = stride - 3;
        while left < right {
            for channel in 0..3 {
                row.swap(left + channel, right + channel);
            }
            left += 3;
            right -= 3;
        }
    }
}

/// The whole frame, oriented and resized to `target`.
fn frame(raster: &Raster, target: Target) -> Result<Rendered, crop::Error> {
    let (width, height) = raster.oriented_size();
    crop::render(
        raster,
        Region {
            x: 0,
            y: 0,
            width,
            height,
        },
        target,
    )
}

fn square(raster: &Raster, side: u32) -> Result<Rendered, crop::Error> {
    frame(raster, Target::square(side))
}

/// BT.601 luma, which is what every sharpness and hash measure here reads.
fn luma(pixels: &[u8]) -> Vec<f32> {
    pixels
        .as_chunks::<3>()
        .0
        .iter()
        .map(|pixel| {
            0.299 * f32::from(pixel[0]) + 0.587 * f32::from(pixel[1]) + 0.114 * f32::from(pixel[2])
        })
        .collect()
}

/// Variance of the Laplacian: a blurred frame has little left after a
/// second-derivative filter, a sharp one keeps its edges.
fn sharpness(luma: &[f32], side: usize) -> f64 {
    // A second derivative needs a pixel on each side of the one it is taken at.
    if side < 3 || luma.len() < side * side {
        return 0.0;
    }
    let mut total = 0.0f64;
    let mut squares = 0.0f64;
    let mut count = 0u32;
    for y in 1..side - 1 {
        for x in 1..side - 1 {
            let centre = luma[y * side + x];
            let response = f64::from(
                luma[(y - 1) * side + x]
                    + luma[(y + 1) * side + x]
                    + luma[y * side + x - 1]
                    + luma[y * side + x + 1]
                    - 4.0 * centre,
            );
            total += response;
            squares += response * response;
            count += 1;
        }
    }
    let mean = total / f64::from(count);
    squares / f64::from(count) - mean * mean
}

/// Mean and standard deviation, which together reject a blown-out, black, or
/// blank frame without needing to look at what is in it.
fn spread(luma: &[f32]) -> (f64, f64) {
    if luma.is_empty() {
        return (0.0, 0.0);
    }
    let mean = f64::from(luma.iter().sum::<f32>()) / luma.len() as f64;
    let variance = luma
        .iter()
        .map(|&value| (f64::from(value) - mean).powi(2))
        .sum::<f64>()
        / luma.len() as f64;
    (mean, variance.sqrt())
}

/// Box-averages a square luma plane down to `side` cells a side.
fn shrink(luma: &[f32], from: usize, side: usize) -> Vec<f32> {
    let block = from / side;
    let mut cells = vec![0.0f32; side * side];
    for y in 0..side {
        for x in 0..side {
            let mut total = 0.0f32;
            for row in 0..block {
                let start = (y * block + row) * from + x * block;
                total += luma[start..start + block].iter().sum::<f32>();
            }
            cells[y * side + x] = total / (block * block) as f32;
        }
    }
    cells
}

/// Difference hash of a 9x8 luma plane: one bit per horizontal neighbour pair.
/// It tracks how a frame is laid out rather than how bright it is, so a shot
/// survives an exposure change but not a change of pose.
fn hash(luma: &[f32]) -> u64 {
    let mut bits = 0u64;
    for y in 0..8 {
        for x in 0..8 {
            let left = luma[y * 9 + x];
            let right = luma[y * 9 + x + 1];
            bits = (bits << 1) | u64::from(left > right);
        }
    }
    bits
}

/// The container a clip is read as, from the extension the browser sent. An
/// extension outside [`CONTAINERS`] is read as MP4, and a clip that is not one
/// then fails to decode rather than being probed as whatever it resembles.
fn container(filename: &str) -> Container {
    let extension = Path::new(filename)
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase);
    CONTAINERS
        .into_iter()
        .find(|container| Some(container.extension) == extension.as_deref())
        .unwrap_or(MP4)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn measured(timestamp_milliseconds: u64, sharpness: f64, hash: u64) -> Measured {
        Measured {
            path: PathBuf::new(),
            timestamp_milliseconds,
            sharpness,
            brightness: 128.0,
            contrast: 40.0,
            luma: vec![0.0; GRID * GRID],
            hash,
        }
    }

    /// A hash far enough from every other one that nothing is taken for a
    /// repeat of anything else.
    fn distinct(index: usize) -> u64 {
        let mut bits = (index as u64).wrapping_add(0x9e37_79b9_7f4a_7c15);
        bits = (bits ^ (bits >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        bits = (bits ^ (bits >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        bits ^ (bits >> 31)
    }

    fn ffmpeg_installed() -> bool {
        std::process::Command::new("ffmpeg")
            .arg("-version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok()
    }

    #[test]
    fn a_blurred_frame_scores_below_a_sharp_one() {
        let side = 32;
        let mut sharp = vec![0.0f32; side * side];
        for y in 0..side {
            for x in 0..side {
                sharp[y * side + x] = if (x / 4 + y / 4) % 2 == 0 { 0.0 } else { 255.0 };
            }
        }
        let mut blurred = sharp.clone();
        for _ in 0..6 {
            let source = blurred.clone();
            for y in 1..side - 1 {
                for x in 1..side - 1 {
                    blurred[y * side + x] = (source[y * side + x]
                        + source[(y - 1) * side + x]
                        + source[(y + 1) * side + x]
                        + source[y * side + x - 1]
                        + source[y * side + x + 1])
                        / 5.0;
                }
            }
        }
        assert!(
            sharpness(&sharp, side) > sharpness(&blurred, side) * 4.0,
            "sharp {} should dwarf blurred {}",
            sharpness(&sharp, side),
            sharpness(&blurred, side)
        );
    }

    #[test]
    fn each_second_keeps_its_sharpest_frames() {
        let frames = vec![
            measured(0, 1.0, distinct(0)),
            measured(250, 9.0, distinct(1)),
            measured(500, 8.0, distinct(2)),
            measured(750, 2.0, distinct(3)),
            measured(1000, 7.0, distinct(4)),
            measured(1250, 1.0, distinct(5)),
            measured(1500, 6.0, distinct(6)),
        ];
        let chosen = choose(&frames, 2, 48);
        assert_eq!(
            chosen,
            vec![1, 2, 4, 6],
            "the blurred frame of each second loses its slot"
        );
    }

    #[test]
    fn a_still_clip_does_not_train_on_one_repeated_pose() {
        let frames: Vec<Measured> = (0..40)
            .map(|index| measured(index as u64 * 100, 1.0, 0x_dead_beef_dead_beef))
            .collect();
        let chosen = choose(&frames, 4, 48);
        assert_eq!(
            chosen.len(),
            FLOOR,
            "identical frames collapse to the floor, not to one"
        );
    }

    #[test]
    fn a_shot_already_kept_loses_its_slot_to_a_new_one() {
        let shots = 12;
        let mut frames: Vec<Measured> = (0..shots)
            .map(|index| measured(index as u64 * 1000, 1.0, distinct(index)))
            .collect();
        for index in 0..shots {
            frames.push(measured(index as u64 * 1000 + 100, 0.5, distinct(index)));
        }
        let chosen = diversify(
            &(0..frames.len()).collect::<Vec<_>>(),
            &frames,
            frames.len(),
        );
        let kept: std::collections::HashSet<u64> =
            chosen.iter().map(|&index| frames[index].hash).collect();
        assert_eq!(chosen.len(), shots, "the budget is spent on distinct shots");
        assert_eq!(kept.len(), shots, "no shot is kept twice");
    }

    #[test]
    fn the_budget_caps_what_a_long_clip_contributes() {
        let frames: Vec<Measured> = (0..200)
            .map(|index| measured(index as u64 * 250, index as f64, distinct(index)))
            .collect();
        let chosen = choose(&frames, 4, 48);
        assert_eq!(chosen.len(), 48);
        assert!(
            chosen
                .windows(2)
                .all(|pair| frames[pair[0]].timestamp_milliseconds
                    <= frames[pair[1]].timestamp_milliseconds),
            "frames come back in time order"
        );
        let last = frames[*chosen.last().unwrap()].timestamp_milliseconds;
        assert!(
            last > 40_000,
            "the whole clip is represented, not just its opening: last frame at {last}ms"
        );
    }

    #[test]
    fn frames_shot_alike_share_a_caption_group() {
        let frames = vec![
            measured(0, 1.0, 0x_0000_0000_0000_0000),
            measured(250, 1.0, 0x_0000_0000_0000_0003),
            measured(500, 1.0, 0x_ffff_ffff_ffff_ffff),
        ];
        assert_eq!(group(&[0, 1, 2], &frames), vec![0, 0, 1]);
    }

    #[test]
    fn the_crop_follows_what_moved() {
        let mut motion = vec![0.0f32; GRID * GRID];
        for y in 2..6 {
            for x in 24..28 {
                motion[y * GRID + x] = 200.0;
            }
        }
        let point = focus(&motion);
        assert!(point.x > 0.7, "focus tracks right: {point:?}");
        assert!(point.y < 0.3, "focus tracks up: {point:?}");
    }

    #[test]
    fn a_still_frame_is_framed_on_its_centre() {
        assert_eq!(
            focus(&vec![0.2; GRID * GRID]),
            Point { x: 0.5, y: 0.5 },
            "noise below the still threshold must not steer the crop"
        );
    }

    #[test]
    fn mirroring_reverses_every_row() {
        let mut rendered = Rendered {
            width: 3,
            height: 2,
            pixels: vec![
                1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18,
            ],
        };
        mirror(&mut rendered);
        assert_eq!(
            rendered.pixels,
            vec![
                7, 8, 9, 4, 5, 6, 1, 2, 3, 16, 17, 18, 13, 14, 15, 10, 11, 12
            ]
        );
    }

    #[test]
    fn an_unnamed_upload_still_gets_a_container() {
        assert_eq!(container("clip.MOV").extension, "mov");
        assert_eq!(container("clip.webm").extension, "webm");
        assert_eq!(container("clip"), MP4);
        assert_eq!(container("clip.../etc/passwd"), MP4);
    }

    #[test]
    fn only_allowlisted_containers_are_demuxed_and_each_by_its_own_demuxer() {
        for (filename, extension, demuxer) in [
            ("a.mp4", "mp4", "mov"),
            ("a.MOV", "mov", "mov"),
            ("a.m4v", "m4v", "mov"),
            ("a.webm", "webm", "matroska"),
            ("a.mkv", "mkv", "matroska"),
            ("a.avi", "avi", "avi"),
        ] {
            assert_eq!(
                container(filename),
                Container { extension, demuxer },
                "{filename}"
            );
        }
        for filename in [
            "playlist.m3u8",
            "list.ffconcat",
            "a.txt",
            "a.hls",
            "a.sdp",
            "a.image2",
            "a.tty",
        ] {
            assert_eq!(container(filename), MP4, "{filename} chose its own demuxer");
        }
    }

    fn position(arguments: &[OsString], flag: &str) -> usize {
        arguments
            .iter()
            .position(|argument| argument == flag)
            .unwrap_or_else(|| panic!("{flag} is missing from {arguments:?}"))
    }

    #[test]
    fn both_tools_open_the_clip_as_a_local_file_with_a_forced_demuxer() {
        let clip = Path::new("/work/clip.webm");
        let container = container("upload.webm");
        for arguments in [
            probe_arguments(container, clip),
            sample_arguments(container, clip, Path::new("/work/frames"), 8.0),
        ] {
            let input = position(&arguments, "-i");
            let whitelist = position(&arguments, "-protocol_whitelist");
            let format = position(&arguments, "-f");
            assert!(
                whitelist < input && format < input,
                "input options must precede -i: {arguments:?}"
            );
            assert_eq!(arguments[whitelist + 1], "file");
            assert_eq!(arguments[format + 1], "matroska");
            assert_eq!(arguments[input + 1], clip.as_os_str());
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_decoder_that_outlives_its_budget_is_killed() {
        use std::os::unix::fs::PermissionsExt;

        let work = tempfile::tempdir().unwrap();
        let finished = work.path().join("finished");
        let decoder = work.path().join("ffmpeg");
        std::fs::write(
            &decoder,
            format!("#!/bin/sh\nsleep 2\ntouch \"{}\"\n", finished.display()),
        )
        .unwrap();
        std::fs::set_permissions(&decoder, std::fs::Permissions::from_mode(0o700)).unwrap();
        let config = Config {
            ffmpeg: decoder.display().to_string(),
            ..Default::default()
        };
        let started = std::time::Instant::now();

        let error = sample(
            &config,
            MP4,
            &work.path().join("clip.mp4"),
            work.path(),
            8.0,
            Duration::from_millis(300),
        )
        .await
        .unwrap_err();

        assert!(matches!(error, TrainError::Invalid(_)), "{error}");
        assert!(started.elapsed() < Duration::from_secs(2));
        tokio::time::sleep(Duration::from_millis(2_300)).await;
        assert!(
            !finished.exists(),
            "the decoder kept running past its budget"
        );
    }

    #[test]
    fn a_clip_ffmpeg_read_nothing_out_of_is_not_a_training_set() {
        let stills = tempfile::tempdir().unwrap();
        let error = build(
            stills.path(),
            8.0,
            Options {
                fps: 4,
                resolution: 512,
                mirror: true,
                limit: 48,
            },
            &Subject::none(),
        )
        .unwrap_err();
        assert!(matches!(error, TrainError::Invalid(_)), "{error}");
    }

    #[tokio::test]
    async fn a_decoder_that_cannot_be_run_is_reported_rather_than_swallowed() {
        // Present but not executable: a misconfigured path, not a missing one,
        // so it is the server's fault in a way "not installed" does not cover.
        let work = tempfile::tempdir().unwrap();
        let blocked = work.path().join("ffmpeg");
        std::fs::write(&blocked, "#!/bin/sh\nexit 0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        let error = extract(
            &Config {
                ffmpeg: blocked.display().to_string(),
                ..Default::default()
            },
            b"clip",
            "clip.mp4",
            Options {
                fps: 4,
                resolution: 512,
                mirror: true,
                limit: 48,
            },
        )
        .await
        .unwrap_err();
        assert!(
            matches!(error, TrainError::Failed(_)),
            "a decoder that is there but unusable is not the same as one that is absent: {error}"
        );
    }

    #[tokio::test]
    async fn an_empty_upload_is_rejected_before_ffmpeg_runs() {
        let error = extract(
            &Config::default(),
            &[],
            "clip.mp4",
            Options {
                fps: 4,
                resolution: 512,
                mirror: true,
                limit: 48,
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(error, TrainError::Invalid(_)), "{error}");
    }

    /// A clip whose subject crosses most of the frame. A crop that ignored the
    /// subject would lose it: the centre 360x360 of this 640x360 frame covers
    /// x 140 to 500, and the ball's centre travels from 105 to 555.
    const MOVING_SUBJECT: &str = "color=c=0x203040:s=640x360:r=30[bg];\
         color=c=0xffcc66:s=70x70:r=30,format=yuva420p,\
         geq=lum='p(X,Y)':a='if(lte(hypot(X-35,Y-35),34),255,0)'[ball];\
         [bg]noise=alls=14:allf=t+u[grain];\
         [grain][ball]overlay=x='70+450*abs(sin(t*0.55))':y='130+75*sin(t*1.3)'";

    /// Where the bright subject sits, as a fraction of the frame from its
    /// centre. `None` when the crop lost it altogether.
    fn subject(frame: &Rendered) -> Option<(f64, f64)> {
        let mut total = 0.0f64;
        let (mut x_moment, mut y_moment) = (0.0f64, 0.0f64);
        for (index, pixel) in frame.pixels.as_chunks::<3>().0.iter().enumerate() {
            let value = 0.299 * f64::from(pixel[0])
                + 0.587 * f64::from(pixel[1])
                + 0.114 * f64::from(pixel[2]);
            if value <= 150.0 {
                continue;
            }
            total += value;
            x_moment += (index % frame.width as usize) as f64 * value;
            y_moment += (index / frame.width as usize) as f64 * value;
        }
        if total == 0.0 {
            return None;
        }
        let centre = f64::from(frame.width - 1) / 2.0;
        Some((
            (x_moment / total - centre) / f64::from(frame.width),
            (y_moment / total - centre) / f64::from(frame.height),
        ))
    }

    async fn synthesize(path: &Path, filter: &str, seconds: &str) {
        let built = Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-y"])
            .args([
                "-filter_complex",
                filter,
                "-t",
                seconds,
                "-pix_fmt",
                "yuv420p",
            ])
            .arg(path)
            .status()
            .await
            .unwrap();
        assert!(built.success(), "could not build the test clip");
    }

    #[test]
    fn a_still_that_cannot_be_read_is_skipped_rather_than_failing_the_clip() {
        let stills = tempfile::tempdir().unwrap();
        let good = crop::render(
            &Raster {
                width: 8,
                height: 8,
                layout: decode::Layout::Rgb,
                orientation: decode::Orientation::Normal,
                pixels: (0..8 * 8)
                    .flat_map(|index| [index as u8, 30, 200])
                    .collect(),
            },
            Region {
                x: 0,
                y: 0,
                width: 8,
                height: 8,
            },
            Target::square(8),
        )
        .unwrap();
        let mut jpeg = Vec::new();
        image::codecs::jpeg::JpegEncoder::new(&mut jpeg)
            .encode(&good.pixels, 8, 8, image::ExtendedColorType::Rgb8)
            .unwrap();

        std::fs::write(stills.path().join("000001.jpg"), &jpeg).unwrap();
        std::fs::write(stills.path().join("000002.jpg"), b"not an image").unwrap();
        std::fs::write(stills.path().join("000003.png"), &jpeg).unwrap();

        let measured = measure(stills.path(), 4.0).unwrap();
        assert_eq!(
            measured.len(),
            1,
            "only the readable jpeg counts: a corrupt frame is dropped, and so is a file \
             ffmpeg did not write"
        );
        assert_eq!(measured[0].timestamp_milliseconds, 0);
    }

    #[test]
    fn a_clip_of_black_frames_still_offers_something_to_choose_from() {
        let frames: Vec<Measured> = (0..12)
            .map(|index| Measured {
                brightness: 1.0,
                contrast: 0.5,
                ..measured(index as u64 * 250, index as f64, distinct(index))
            })
            .collect();
        assert!(
            !choose(&frames, 4, 48).is_empty(),
            "a clip too dark to judge is still the clip the caller submitted"
        );
    }

    #[test]
    fn one_candidate_needs_no_ranking() {
        let frames = vec![measured(0, 1.0, distinct(0))];
        assert_eq!(diversify(&[0], &frames, 48), vec![0]);
        assert_eq!(diversify(&[], &frames, 48), Vec::<usize>::new());
    }

    #[test]
    fn an_empty_plane_has_no_spread() {
        assert_eq!(spread(&[]), (0.0, 0.0));
        assert_eq!(sharpness(&[], 0), 0.0);
    }

    #[test]
    fn a_frame_with_no_motion_at_all_is_framed_on_its_centre() {
        assert_eq!(focus(&vec![0.0; GRID * GRID]), Point { x: 0.5, y: 0.5 });
    }

    #[test]
    fn a_single_frame_clip_has_motion_to_read() {
        let frames = vec![measured(0, 1.0, distinct(0))];
        let motion = motion(&frames);
        assert_eq!(motion.len(), 1);
        assert_eq!(motion[0].len(), GRID * GRID);
    }

    #[tokio::test]
    async fn a_missing_decoder_reads_as_unconfigured_not_as_a_bad_clip() {
        let error = extract(
            &Config {
                ffmpeg: "no-such-decoder-on-this-machine".into(),
                ..Default::default()
            },
            b"clip",
            "clip.mp4",
            Options {
                fps: 4,
                resolution: 512,
                mirror: true,
                limit: 48,
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(error, TrainError::Disabled), "{error}");
    }

    #[tokio::test]
    async fn a_file_that_holds_no_video_is_the_callers_problem() {
        if !ffmpeg_installed() {
            eprintln!("skipping: ffmpeg is not installed");
            return;
        }
        let error = extract(
            &Config::default(),
            b"this is not a video",
            "notes.txt",
            Options {
                fps: 4,
                resolution: 512,
                mirror: true,
                limit: 48,
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(error, TrainError::Invalid(_)), "{error}");
    }

    #[tokio::test]
    async fn a_clip_becomes_square_training_frames() {
        if !ffmpeg_installed() {
            eprintln!("skipping: ffmpeg is not installed");
            return;
        }
        let work = tempfile::tempdir().unwrap();
        let clip = work.path().join("clip.mp4");
        synthesize(&clip, MOVING_SUBJECT, "3").await;

        let extracted = extract(
            &Config::default(),
            &std::fs::read(&clip).unwrap(),
            "clip.mp4",
            Options {
                fps: 3,
                resolution: 256,
                mirror: true,
                limit: 48,
            },
        )
        .await
        .unwrap();

        assert!(
            extracted.sampled > extracted.frames.len(),
            "sampling runs above the kept rate: {} sampled, {} kept",
            extracted.sampled,
            extracted.frames.len()
        );
        assert!(
            (6..=9).contains(&extracted.frames.len()),
            "three seconds at three frames a second: {}",
            extracted.frames.len()
        );
        assert!(
            extracted.frames.iter().any(|frame| frame.mirrored)
                && extracted.frames.iter().any(|frame| !frame.mirrored),
            "half of each second is mirrored"
        );
        assert!(
            extracted
                .frames
                .windows(2)
                .all(|pair| pair[0].timestamp_milliseconds < pair[1].timestamp_milliseconds),
            "frames arrive in time order"
        );
        for frame in &extracted.frames {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(&frame.bytes_base64)
                .unwrap();
            let raster = decode::decode(&bytes).unwrap();
            assert_eq!(
                raster.oriented_size(),
                (256, 256),
                "{} is not the square the trainer wants",
                frame.filename
            );
            let (x, y) = subject(&square(&raster, 256).unwrap())
                .unwrap_or_else(|| panic!("{} cropped the subject out", frame.filename));
            assert!(
                x.abs() < 0.35 && y.abs() < 0.35,
                "{} framed the subject at {x:+.2}, {y:+.2} instead of near its centre",
                frame.filename
            );
        }
    }

    #[tokio::test]
    async fn a_clip_is_demuxed_as_the_container_it_is_named_rather_than_probed() {
        if !ffmpeg_installed() {
            eprintln!("skipping: ffmpeg is not installed");
            return;
        }
        let work = tempfile::tempdir().unwrap();
        let clip = work.path().join("clip.webm");
        synthesize(&clip, MOVING_SUBJECT, "1").await;
        let bytes = std::fs::read(&clip).unwrap();
        let options = Options {
            fps: 2,
            resolution: 64,
            mirror: false,
            limit: 4,
        };

        extract(&Config::default(), &bytes, "clip.webm", options)
            .await
            .expect("a WebM named as one is read as Matroska");
        let error = extract(&Config::default(), &bytes, "clip.mp4", options)
            .await
            .expect_err("a WebM named as MP4 must not be probed into being read anyway");
        assert!(matches!(error, TrainError::Invalid(_)), "{error}");
    }

    #[tokio::test]
    async fn mirroring_can_be_turned_off_for_a_subject_a_mirror_would_get_wrong() {
        if !ffmpeg_installed() {
            eprintln!("skipping: ffmpeg is not installed");
            return;
        }
        let work = tempfile::tempdir().unwrap();
        let clip = work.path().join("clip.mp4");
        synthesize(&clip, MOVING_SUBJECT, "2").await;

        let extracted = extract(
            &Config::default(),
            &std::fs::read(&clip).unwrap(),
            "clip.mp4",
            Options {
                fps: 3,
                resolution: 128,
                mirror: false,
                limit: 48,
            },
        )
        .await
        .unwrap();
        assert!(!extracted.frames.is_empty());
        assert!(
            extracted.frames.iter().all(|frame| !frame.mirrored),
            "a subject carrying text must come back the way round it was filmed"
        );
    }

    #[tokio::test]
    async fn a_clip_of_nothing_happening_still_yields_a_trainable_set() {
        if !ffmpeg_installed() {
            eprintln!("skipping: ffmpeg is not installed");
            return;
        }
        let work = tempfile::tempdir().unwrap();
        let clip = work.path().join("still.mp4");
        synthesize(
            &clip,
            "color=c=0x606060:s=320x240:r=30,drawbox=x=100:y=70:w=120:h=100:color=white@1:t=fill",
            "3",
        )
        .await;

        let extracted = extract(
            &Config::default(),
            &std::fs::read(&clip).unwrap(),
            "still.mp4",
            Options {
                fps: 4,
                resolution: 128,
                mirror: false,
                limit: 48,
            },
        )
        .await
        .unwrap();
        assert_eq!(
            extracted.frames.len(),
            FLOOR,
            "twenty identical frames are one pose, but a set of one cannot train"
        );
    }
}
