use serde::Deserialize;
use serde::Serialize;

/// The stretch of a source video one training clip covers, and the pose it
/// shows, as the `NNNN.json` beside each `NNNN.mp4` in the host job's
/// [`CLIPS_DIRECTORY`](crate::host::CLIPS_DIRECTORY) holds it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ClipWindow {
    /// Where the window starts in its source, in seconds.
    #[serde(rename = "start_s")]
    pub start: f64,
    /// Where the window ends in its source, in seconds.
    #[serde(rename = "end_s")]
    pub end: f64,
    /// The [`pose_cluster`](crate::dataset::pose_cluster) the window shows.
    pub pose: String,
}

impl ClipWindow {
    /// The window from `start` to `end` seconds of its source, showing
    /// `pose`.
    pub fn new(start: f64, end: f64, pose: impl Into<String>) -> Self {
        Self {
            start,
            end,
            pose: pose.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_window_keeps_the_wire_names_the_trainer_reads() {
        let window = ClipWindow::new(1.5, 4.0, "standing");
        let encoded = serde_json::to_value(&window).unwrap();
        assert_eq!(
            encoded,
            serde_json::json!({"start_s": 1.5, "end_s": 4.0, "pose": "standing"})
        );
        assert_eq!(
            serde_json::from_value::<ClipWindow>(encoded).unwrap(),
            window
        );
    }
}
