use serde::Deserialize;
use std::fmt;

/// A clip submitted for training.
#[derive(Deserialize)]
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

impl fmt::Debug for FrameRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FrameRequest")
            .field("filename", &self.filename)
            .field("bytes_base64", &self.bytes_base64.len())
            .field("fps", &self.fps)
            .field("mirror", &self.mirror)
            .finish()
    }
}
