use serde::Serialize;
use std::fmt;

/// One training image pulled from a clip.
#[derive(Clone, Serialize)]
#[non_exhaustive]
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
