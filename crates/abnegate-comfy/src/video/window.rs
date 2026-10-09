use super::CEILING;
use super::Container;
use super::DECODE_TIMEOUT;
use super::Measured;
use super::PROBE_TIMEOUT;
use super::container;
use super::demuxer_arguments;
use super::duration;
use super::execute;
use super::group;
use super::measure;
use super::opened;
use super::sample;
use crate::config::Config;
use crate::dataset;
use crate::dataset::ClipWindow;
use crate::lora::TrainError;
use std::collections::HashSet;
use std::ffi::OsString;
use std::io;
use std::path::Path;

/// Width of a window, the width the packaged Wan graphs generate at.
const WIDTH: u32 = 832;
/// Height of a window, the height the packaged Wan graphs generate at.
const HEIGHT: u32 = 480;
/// Frames in a window, as many as one generated clip holds.
const FRAMES: u32 = 49;
/// Frames per second a window is written at, the rate a generated clip plays
/// at.
const RATE: u32 = 24;
/// Pose changes a clip needs before its windows follow them rather than
/// being spread evenly.
const MINIMUM_SHIFTS: usize = 2;
/// Most windows one clip is cut into: the first pose, the last, and the one
/// nearest the middle of the two.
const MAXIMUM_WINDOWS: usize = 3;
/// Slowest rate a clip is sampled at to find where its pose changes.
const SLOWEST_ANALYSIS: f64 = 0.5;
/// Fastest rate a clip is sampled at to find where its pose changes.
const FASTEST_ANALYSIS: f64 = 2.0;
/// A clip shorter than this many windows gets one window.
const ONE_WINDOW_BELOW: f64 = 1.25;
/// A clip shorter than this many windows gets two; a longer one, three.
const TWO_WINDOWS_BELOW: f64 = 2.5;

/// Cuts `video` into the clip windows a video adapter trains on and writes
/// each into `destination` as `NNNN.mp4` beside its [`ClipWindow`] in
/// `NNNN.json`, numbering from `start`. `filename`'s extension picks the
/// container the clip is read as, as [`extract`](super::extract) reads it.
///
/// Fails with [`TrainError::Invalid`] for an empty clip, one ffprobe reads no
/// duration from, or one ffmpeg cannot cut, [`TrainError::Disabled`] when
/// ffprobe or ffmpeg is not installed, and [`TrainError::Failed`] when either
/// cannot be run or a window cannot be written.
pub(crate) async fn stage(
    config: &Config,
    video: &[u8],
    filename: &str,
    destination: &Path,
    start: usize,
) -> Result<Vec<ClipWindow>, TrainError> {
    if video.is_empty() {
        return Err(TrainError::Invalid("video is empty"));
    }
    let work = tempfile::tempdir().map_err(failed)?;
    let container = container(filename);
    let clip = work.path().join(format!("clip.{}", container.extension));
    tokio::fs::write(&clip, video).await.map_err(failed)?;
    let seconds = duration(config, container, &clip, PROBE_TIMEOUT)
        .await
        .map_err(unrunnable)?
        .filter(|seconds| seconds.is_finite() && *seconds > 0.0)
        .ok_or(TrainError::Invalid("video has no readable duration"))?;
    let shifts = shifts(config, container, &clip, work.path(), seconds).await;
    let windows = plan(seconds, &shifts);
    if windows.is_empty() {
        return Err(TrainError::Invalid("video has no usable clip windows"));
    }
    std::fs::create_dir_all(destination).map_err(failed)?;
    for (offset, window) in windows.iter().enumerate() {
        let stem = format!("{:04}", start + offset);
        transcode(
            config,
            container,
            &clip,
            &dataset::clip_path(destination, &stem),
            window,
        )
        .await?;
        describe(destination, &stem, window)?;
    }
    Ok(windows)
}

/// The length of one window, in seconds.
fn length() -> f64 {
    f64::from(FRAMES) / f64::from(RATE)
}

/// The windows over a clip `duration` seconds long. With at least
/// [`MINIMUM_SHIFTS`] distinct poses among `shifts`, the moments each pose
/// first appears, and room for more than one window, the windows centre on
/// the ones [`pick`] keeps; otherwise they are spread evenly, as many as
/// [`count`] gives.
fn plan(duration: f64, shifts: &[(f64, String)]) -> Vec<ClipWindow> {
    if duration.is_nan() || duration <= 0.0 {
        return Vec::new();
    }
    let length = length().min(duration);
    let unique = unique(duration, shifts);
    if unique.len() >= MINIMUM_SHIFTS && count(duration, length) > 1 {
        return pick(unique)
            .into_iter()
            .map(|(at, pose)| centred(at, length, duration, pose))
            .collect();
    }
    let pose = unique
        .first()
        .map(|(_, pose)| pose.clone())
        .unwrap_or_default();
    spread(duration, length, count(duration, length), &pose)
}

/// The first moment each pose appears inside the clip, in the order given.
fn unique(duration: f64, shifts: &[(f64, String)]) -> Vec<(f64, String)> {
    let mut unique: Vec<(f64, String)> = Vec::new();
    for (at, pose) in shifts {
        if !(0.0..=duration).contains(at) || unique.iter().any(|(_, seen)| seen == pose) {
            continue;
        }
        unique.push((*at, pose.clone()));
    }
    unique
}

/// At most [`MAXIMUM_WINDOWS`] of `unique`, in time order: the first, the
/// last, and the one nearest the middle of the two.
fn pick(mut unique: Vec<(f64, String)>) -> Vec<(f64, String)> {
    unique.sort_by(|left, right| left.0.total_cmp(&right.0));
    if unique.len() <= MAXIMUM_WINDOWS {
        return unique;
    }
    let first = unique.remove(0);
    let Some(last) = unique.pop() else {
        return vec![first];
    };
    let middle = (first.0 + last.0) / 2.0;
    let nearest = unique
        .into_iter()
        .min_by(|left, right| (left.0 - middle).abs().total_cmp(&(right.0 - middle).abs()));
    let mut kept = vec![first];
    kept.extend(nearest);
    kept.push(last);
    kept.sort_by(|left, right| left.0.total_cmp(&right.0));
    kept
}

/// How many evenly spread windows a clip `duration` seconds long gets.
fn count(duration: f64, length: f64) -> usize {
    if duration < length * ONE_WINDOW_BELOW {
        1
    } else if duration < length * TWO_WINDOWS_BELOW {
        2
    } else {
        MAXIMUM_WINDOWS
    }
}

/// A window `length` seconds long centred on `at`, moved back inside a clip
/// `duration` seconds long when it would run past either end.
fn centred(at: f64, length: f64, duration: f64, pose: String) -> ClipWindow {
    let mut start = (at - length / 2.0).max(0.0);
    let mut end = start + length;
    if end > duration {
        end = duration;
        start = (end - length).max(0.0);
    }
    ClipWindow::new(start, end, pose)
}

/// `count` windows `length` seconds long, the first at the start of the clip
/// and the last at its end.
fn spread(duration: f64, length: f64, count: usize, pose: &str) -> Vec<ClipWindow> {
    if count <= 1 {
        return vec![ClipWindow::new(0.0, length.min(duration), pose)];
    }
    let span = (duration - length).max(0.0);
    (0..count)
        .map(|index| {
            let start = span * index as f64 / (count - 1) as f64;
            ClipWindow::new(start, (start + length).min(duration), pose)
        })
        .collect()
}

/// Where the pose of the clip changes: the moment each shot first appears,
/// read from stills sampled into `work`, named `pose-<shot>`. None when the
/// clip cannot be sampled, which spreads the windows evenly instead.
async fn shifts(
    config: &Config,
    container: Container,
    clip: &Path,
    work: &Path,
    seconds: f64,
) -> Vec<(f64, String)> {
    let stills = work.join("analysis");
    if std::fs::create_dir_all(&stills).is_err() {
        return Vec::new();
    }
    let rate = analysis_rate(seconds);
    if sample(config, container, clip, &stills, rate, DECODE_TIMEOUT)
        .await
        .is_err()
    {
        return Vec::new();
    }
    tokio::task::spawn_blocking(move || {
        measure(&stills, rate)
            .map(|measured| first_appearances(&measured))
            .unwrap_or_default()
    })
    .await
    .unwrap_or_default()
}

/// How many stills a second a clip `seconds` long is sampled at to find
/// where its pose changes: as many as [`CEILING`] spreads over it, within
/// [`SLOWEST_ANALYSIS`] and [`FASTEST_ANALYSIS`]. A clip too long for even
/// the slowest rate is still sampled at most [`CEILING`] times, from its
/// start.
fn analysis_rate(seconds: f64) -> f64 {
    (CEILING as f64 / seconds).clamp(SLOWEST_ANALYSIS, FASTEST_ANALYSIS)
}

/// The moment each shot among `measured` first appears, every shot of the
/// clip, so [`pick`] can keep the last one.
fn first_appearances(measured: &[Measured]) -> Vec<(f64, String)> {
    let indices: Vec<usize> = (0..measured.len()).collect();
    let mut seen = HashSet::new();
    let mut shifts = Vec::new();
    for (index, shot) in indices.iter().zip(group(&indices, measured)) {
        if !seen.insert(shot) {
            continue;
        }
        shifts.push((
            measured[*index].timestamp_milliseconds as f64 / 1000.0,
            format!("pose-{shot}"),
        ));
    }
    shifts
}

/// Writes `window` as the description of the clip window `stem` in
/// `destination`.
fn describe(destination: &Path, stem: &str, window: &ClipWindow) -> Result<(), TrainError> {
    let encoded =
        serde_json::to_vec_pretty(window).map_err(|error| TrainError::Failed(error.to_string()))?;
    std::fs::write(dataset::window_path(destination, stem), encoded).map_err(failed)
}

async fn transcode(
    config: &Config,
    container: Container,
    source: &Path,
    destination: &Path,
    window: &ClipWindow,
) -> Result<(), TrainError> {
    let output = execute(
        &config.ffmpeg,
        &transcode_arguments(container, source, destination, window),
        DECODE_TIMEOUT,
    )
    .await
    .map_err(unrunnable)?
    .ok_or(TrainError::Invalid(
        "the video took too long to decode; try a shorter clip",
    ))?;
    if !output.status.success() {
        tracing::warn!(
            reason = %crate::excerpt::tail(&String::from_utf8_lossy(&output.stderr)),
            "ffmpeg could not cut a clip window"
        );
        return Err(TrainError::Invalid(
            "the video could not be decoded; try re-exporting it as H.264 MP4",
        ));
    }
    Ok(())
}

/// Cuts `window` out of `source`, letterboxed to the generated size at the
/// generated rate, as silent H.264. The window is sought on the input, so
/// ffmpeg skips to it rather than decoding everything before it, and the cut
/// still starts on the frame asked for because it is re-encoded.
fn transcode_arguments(
    container: Container,
    source: &Path,
    destination: &Path,
    window: &ClipWindow,
) -> Vec<OsString> {
    let filter = format!(
        "scale={WIDTH}:{HEIGHT}:force_original_aspect_ratio=decrease,\
         pad={WIDTH}:{HEIGHT}:(ow-iw)/2:(oh-ih)/2,fps={RATE}"
    );
    let start = format!("{:.3}", window.start);
    let length = format!(
        "{:.3}",
        (window.end - window.start).max(1.0 / f64::from(RATE))
    );
    let frames = FRAMES.to_string();
    let mut arguments: Vec<OsString> = ["-hide_banner", "-nostdin", "-loglevel", "error", "-y"]
        .into_iter()
        .map(OsString::from)
        .collect();
    arguments.extend(demuxer_arguments(container));
    arguments.extend([OsString::from("-ss"), OsString::from(&start)]);
    arguments.extend(opened(source));
    arguments.extend(
        [
            "-t",
            &length,
            "-vf",
            &filter,
            "-frames:v",
            &frames,
            "-an",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
        ]
        .into_iter()
        .map(OsString::from),
    );
    arguments.push(destination.as_os_str().to_os_string());
    arguments
}

fn failed(error: io::Error) -> TrainError {
    TrainError::Failed(error.to_string())
}

/// Why a decoder could not be run: [`TrainError::Disabled`] when it is not
/// installed.
fn unrunnable(error: io::Error) -> TrainError {
    match error.kind() {
        io::ErrorKind::NotFound => TrainError::Disabled,
        _ => failed(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::process::Stdio;
    use tokio::process::Command;

    #[test]
    fn a_clip_shorter_than_one_window_gets_one_window_however_it_poses() {
        let windows = plan(1.0, &shifts(&[(0.1, "standing"), (0.6, "sitting")]));
        assert_eq!(windows.len(), 1, "{windows:?}");
    }

    fn shifts(moments: &[(f64, &str)]) -> Vec<(f64, String)> {
        moments
            .iter()
            .map(|(at, pose)| (*at, (*pose).to_string()))
            .collect()
    }

    fn measured(timestamp_milliseconds: u64, hash: u64) -> Measured {
        Measured {
            path: PathBuf::new(),
            timestamp_milliseconds,
            sharpness: 1.0,
            brightness: 128.0,
            contrast: 40.0,
            luma: Vec::new(),
            hash,
        }
    }

    fn ffmpeg_installed() -> bool {
        ["ffmpeg", "ffprobe"].iter().all(|tool| {
            std::process::Command::new(tool)
                .arg("-version")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|status| status.success())
        })
    }

    #[test]
    fn windows_cover_short_clips_whole_and_spread_over_long_ones() {
        let short = plan(2.0, &[]);
        assert_eq!(short.len(), 1);
        assert!((short[0].end - short[0].start - 2.0).abs() < 0.05);

        let pair = plan(5.0, &[]);
        assert_eq!(pair.len(), 2);
        for window in &pair {
            assert!((window.end - window.start - length()).abs() < 0.05);
        }

        let spread = plan(12.0, &[]);
        assert_eq!(spread.len(), 3);
        assert!(spread[0].start < spread[1].start);
        assert!(spread[1].start < spread[2].start);
        assert!(
            (spread[2].end - 12.0).abs() < 1e-9,
            "the last window ends the clip"
        );

        assert!(plan(0.0, &[]).is_empty());
        assert!(plan(f64::NAN, &[]).is_empty());
    }

    #[test]
    fn windows_follow_the_moments_the_pose_changes() {
        let windows = plan(
            12.0,
            &shifts(&[
                (1.0, "standing"),
                (2.0, "standing"),
                (6.0, "sitting"),
                (10.0, "walking"),
                (11.0, "lying"),
            ]),
        );
        assert_eq!(windows.len(), 3);
        let poses: Vec<&str> = windows.iter().map(|window| window.pose.as_str()).collect();
        assert_eq!(poses, ["standing", "sitting", "lying"]);
        for window in &windows {
            assert!(window.start >= 0.0);
            assert!(window.end <= 12.0 + 1e-3);
            assert!(window.end > window.start);
        }

        let one_pose = plan(12.0, &shifts(&[(1.0, "standing"), (5.0, "standing")]));
        assert_eq!(one_pose.len(), 3, "one pose is spread evenly");
        assert!(one_pose.iter().all(|window| window.pose == "standing"));

        let outside = plan(4.0, &shifts(&[(-1.0, "standing"), (9.0, "sitting")]));
        assert_eq!(outside.len(), 2, "a change outside the clip is ignored");
        assert!(outside.iter().all(|window| window.pose.is_empty()));
    }

    #[test]
    fn each_shot_shifts_the_pose_once() {
        let far = |index: u64| index.wrapping_mul(0x9e37_79b9_7f4a_7c15);
        let frames = [
            measured(0, far(1)),
            measured(500, far(1)),
            measured(1000, far(2)),
            measured(1500, far(1)),
            measured(2000, far(3)),
            measured(2500, far(4)),
        ];
        assert_eq!(
            first_appearances(&frames),
            shifts(&[
                (0.0, "pose-0"),
                (1.0, "pose-1"),
                (2.0, "pose-2"),
                (2.5, "pose-3")
            ])
        );
        assert!(first_appearances(&[]).is_empty());
    }

    #[test]
    fn the_last_window_comes_from_the_last_shot() {
        let far = |index: u64| index.wrapping_mul(0x9e37_79b9_7f4a_7c15);
        let frames: Vec<Measured> = (0..5)
            .map(|shot| measured(shot * 1000, far(shot + 1)))
            .collect();

        let picked = pick(first_appearances(&frames));

        assert_eq!(picked.len(), MAXIMUM_WINDOWS);
        assert_eq!(picked.first().unwrap().1, "pose-0");
        assert_eq!(picked.last().unwrap().1, "pose-4", "{picked:?}");
    }

    #[test]
    fn a_window_is_described_beside_its_clip_where_the_trainer_reads_it() {
        let root = tempfile::tempdir().unwrap();
        let windows = plan(
            8.0,
            &shifts(&[(1.0, "standing"), (4.0, "sitting"), (7.0, "walking")]),
        );
        assert_eq!(windows.len(), 3);
        for (index, window) in windows.iter().enumerate() {
            describe(root.path(), &format!("{index:04}"), window).unwrap();
        }
        let loaded: ClipWindow =
            serde_json::from_slice(&std::fs::read(root.path().join("0002.json")).unwrap()).unwrap();
        assert_eq!(loaded.pose, "walking");
        assert_eq!(loaded, windows[2]);
    }

    #[test]
    fn a_window_is_cut_from_a_local_file_at_the_generated_size_and_length() {
        let window = ClipWindow::new(1.5, 3.5, "standing");
        let arguments = transcode_arguments(
            container("walk.webm"),
            Path::new("/work/clip.webm"),
            Path::new("/job/clips/0000.mp4"),
            &window,
        );
        let text: Vec<String> = arguments
            .iter()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect();
        let position = |flag: &str| text.iter().position(|argument| argument == flag).unwrap();
        let after = |flag: &str| text[position(flag) + 1].clone();
        assert_eq!(after("-protocol_whitelist"), "file");
        assert_eq!(after("-f"), "matroska");
        assert_eq!(after("-ss"), "1.500");
        assert_eq!(after("-t"), "2.000");
        assert_eq!(after("-frames:v"), "49");
        assert!(after("-vf").starts_with("scale=832:480:"));
        assert!(after("-vf").ends_with("fps=24"));
        assert!(
            position("-f") < position("-ss") && position("-ss") < position("-i"),
            "the window is sought on the input, so ffmpeg skips to it rather than decoding \
             the clip from its start"
        );
        assert_eq!(text.last().unwrap(), "/job/clips/0000.mp4");
    }

    #[test]
    fn a_long_clip_is_sampled_no_more_than_the_ceiling_allows() {
        let two_hours = 7200.0;
        let rate = analysis_rate(two_hours);
        assert_eq!(rate, SLOWEST_ANALYSIS);
        assert!(rate * two_hours > CEILING as f64);
        let arguments: Vec<String> = super::super::sample_arguments(
            container("walk.mp4"),
            Path::new("/work/clip.mp4"),
            Path::new("/work/analysis"),
            rate,
        )
        .iter()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect();
        let frames = arguments
            .iter()
            .position(|argument| argument == "-frames:v")
            .expect("the stills are capped");
        assert_eq!(arguments[frames + 1], CEILING.to_string());
        assert_eq!(analysis_rate(1.0), FASTEST_ANALYSIS);
    }

    #[tokio::test]
    async fn a_missing_ffprobe_disables_video_training() {
        let root = tempfile::tempdir().unwrap();
        let config = Config {
            ffprobe: root.path().join("ffprobe").to_string_lossy().into_owned(),
            ..Config::default()
        };
        let staged = stage(&config, b"a clip", "walk.mp4", root.path(), 0).await;
        assert!(matches!(staged, Err(TrainError::Disabled)), "{staged:?}");
    }

    #[tokio::test]
    async fn an_empty_clip_is_refused_before_anything_runs() {
        let root = tempfile::tempdir().unwrap();
        assert!(matches!(
            stage(&Config::default(), &[], "walk.mp4", root.path(), 0).await,
            Err(TrainError::Invalid("video is empty"))
        ));
        assert!(
            std::fs::read_dir(root.path()).unwrap().next().is_none(),
            "nothing was written"
        );
    }

    #[tokio::test]
    async fn a_clip_is_cut_into_numbered_windows_the_trainer_can_read() {
        if !ffmpeg_installed() {
            eprintln!("skipping: ffmpeg is not installed");
            return;
        }
        let work = tempfile::tempdir().unwrap();
        let source = work.path().join("walk.mp4");
        let built = Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-y"])
            .args([
                "-f",
                "lavfi",
                "-i",
                "testsrc=size=320x240:rate=30",
                "-t",
                "6",
                "-pix_fmt",
                "yuv420p",
            ])
            .arg(&source)
            .status()
            .await
            .unwrap();
        assert!(built.success(), "could not build the test clip");
        let clips = work.path().join("clips");

        let windows = stage(
            &Config::default(),
            &std::fs::read(&source).unwrap(),
            "walk.mp4",
            &clips,
            3,
        )
        .await
        .expect("a six-second clip is cut into windows");

        assert!(
            (2..=MAXIMUM_WINDOWS).contains(&windows.len()),
            "{windows:?}"
        );
        for (offset, window) in windows.iter().enumerate() {
            let stem = format!("{:04}", 3 + offset);
            let clip = dataset::clip_path(&clips, &stem);
            assert!(
                std::fs::metadata(&clip).unwrap().len() > 0,
                "{} is empty",
                clip.display()
            );
            let described: ClipWindow = serde_json::from_slice(
                &std::fs::read(dataset::window_path(&clips, &stem)).unwrap(),
            )
            .unwrap();
            assert_eq!(&described, window);
            assert!(window.end <= 6.0 + 1e-3 && window.start >= 0.0);
        }
        let probed = Command::new("ffprobe")
            .args([
                "-v",
                "error",
                "-select_streams",
                "v:0",
                "-show_entries",
                "stream=width,height",
                "-of",
                "csv=p=0",
            ])
            .arg(dataset::clip_path(&clips, "0003"))
            .output()
            .await
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&probed.stdout).trim(), "832,480");
    }
}
