use crate::client::Error;
use std::fmt;
use uuid::Uuid;

/// Clips come back from the artifact store rather than a chat upload, so the
/// cap matches what the store is willing to keep rather than a request body.
pub const MAX_SOURCE_VIDEO_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone)]
pub struct SourceVideo {
    pub bytes: bytes::Bytes,
    pub mime: String,
    pub filename: String,
}

impl fmt::Debug for SourceVideo {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SourceVideo")
            .field("bytes", &self.bytes.len())
            .field("mime", &self.mime)
            .field("filename", &self.filename)
            .finish()
    }
}

impl SourceVideo {
    pub fn new(bytes: impl Into<bytes::Bytes>, mime: &str) -> Result<Self, Error> {
        let mime = normalize_source_video_mime(mime)?;
        let bytes = bytes.into();
        if bytes.is_empty() || bytes.len() > MAX_SOURCE_VIDEO_BYTES {
            return Err(Error::Configuration("source video is empty or too large"));
        }
        Ok(Self {
            filename: format!(
                "upscale-{}.{}",
                Uuid::new_v4(),
                extension_for_video_mime(&mime)
            ),
            bytes,
            mime,
        })
    }
}

fn normalize_source_video_mime(mime: &str) -> Result<String, Error> {
    match mime.trim().to_ascii_lowercase().as_str() {
        "video/webm" => Ok("video/webm".to_string()),
        "video/mp4" => Ok("video/mp4".to_string()),
        _ => Err(Error::Configuration("source video type is not supported")),
    }
}

fn extension_for_video_mime(mime: &str) -> &'static str {
    match mime {
        "video/mp4" => "mp4",
        _ => "webm",
    }
}
