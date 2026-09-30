use serde::Serialize;
use std::fmt;

/// One training image pulled from a clip.
#[derive(Clone, Serialize)]
#[non_exhaustive]
pub struct Frame {
    /// A name for the frame, `frame-NNNN.png` by position.
    pub filename: String,
    /// The cropped frame as PNG, in standard base64.
    pub bytes_base64: String,
    /// Where in the clip it was taken, in milliseconds. `timestamp_ms` on the
    /// wire.
    #[serde(rename = "timestamp_ms")]
    pub timestamp_milliseconds: u64,
    /// Whether it was flipped left to right.
    pub mirrored: bool,
    /// Frames sharing a group are the same shot, so one caption describes them
    /// all and the vision model only has to look at one of them.
    pub group: usize,
}

impl fmt::Debug for Frame {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Frame")
            .field("filename", &self.filename)
            .field("bytes_base64", &self.bytes_base64.len())
            .field("timestamp_milliseconds", &self.timestamp_milliseconds)
            .field("mirrored", &self.mirrored)
            .field("group", &self.group)
            .finish()
    }
}
