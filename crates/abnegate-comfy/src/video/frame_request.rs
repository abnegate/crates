use crate::lora::TrainError;
use base64::Engine;
use serde::Deserialize;
use std::borrow::Cow;
use std::fmt;

/// A clip submitted for training, as base64 from a JSON body or as raw bytes
/// from a multipart upload. The raw bytes win when both are given.
#[derive(Deserialize)]
#[non_exhaustive]
pub struct FrameRequest {
    /// The clip's name, whose extension picks the container it is read as.
    pub filename: String,
    /// The encoded clip, in standard base64.
    #[serde(default)]
    pub bytes_base64: String,
    /// The encoded clip, as uploaded.
    #[serde(skip)]
    pub bytes: Option<Vec<u8>>,
    /// Frames kept per second. Falls back to the configured rate.
    #[serde(default)]
    pub fps: Option<u32>,
    /// Mirror alternate frames. Worth turning off for a subject carrying text
    /// or anything else a mirror would render backwards.
    #[serde(default)]
    pub mirror: Option<bool>,
}

impl FrameRequest {
    /// The clip `filename` from its base64 `bytes_base64`, at the configured
    /// rate and mirroring.
    pub fn new(filename: impl Into<String>, bytes_base64: impl Into<String>) -> Self {
        Self {
            filename: filename.into(),
            bytes_base64: bytes_base64.into(),
            bytes: None,
            fps: None,
            mirror: None,
        }
    }

    /// The clip `filename` from its encoded `bytes` as uploaded, at the
    /// configured rate and mirroring.
    pub fn from_bytes(filename: impl Into<String>, bytes: impl Into<Vec<u8>>) -> Self {
        Self {
            bytes: Some(bytes.into()),
            ..Self::new(filename, String::new())
        }
    }

    /// Keeps `fps` frames per second instead of the configured rate.
    pub fn with_fps(mut self, fps: u32) -> Self {
        self.fps = Some(fps);
        self
    }

    /// Sets whether alternate frames are mirrored.
    pub fn with_mirror(mut self, mirror: bool) -> Self {
        self.mirror = Some(mirror);
        self
    }

    /// The encoded clip: the uploaded bytes, borrowed, or else the base64
    /// decoded. Fails with [`TrainError::Invalid`] when it is empty or not
    /// base64.
    pub fn encoded(&self) -> Result<Cow<'_, [u8]>, TrainError> {
        let video = match &self.bytes {
            Some(bytes) => Cow::Borrowed(bytes.as_slice()),
            None => Cow::Owned(
                base64::engine::general_purpose::STANDARD
                    .decode(self.bytes_base64.trim())
                    .map_err(|_| TrainError::Invalid("video is not valid base64"))?,
            ),
        };
        if video.is_empty() {
            return Err(TrainError::Invalid("video is empty"));
        }
        Ok(video)
    }
}

impl fmt::Debug for FrameRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FrameRequest")
            .field("filename", &self.filename)
            .field("bytes_base64", &self.bytes_base64.len())
            .field("bytes", &self.bytes.as_ref().map(Vec::len))
            .field("fps", &self.fps)
            .field("mirror", &self.mirror)
            .finish()
    }
}
