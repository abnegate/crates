use crate::client::Error;
use crate::media::MediaType;
use std::fmt;
use uuid::Uuid;

pub const MAXIMUM_SOURCE_IMAGE_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone)]
pub struct SourceImage {
    pub bytes: bytes::Bytes,
    pub mime: String,
    pub filename: String,
}

impl fmt::Debug for SourceImage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SourceImage")
            .field("bytes", &self.bytes.len())
            .field("mime", &self.mime)
            .field("filename", &self.filename)
            .finish()
    }
}

impl SourceImage {
    pub fn new(bytes: impl Into<bytes::Bytes>, mime: &str) -> Result<Self, Error> {
        let mime = normalize_source_mime(mime)?;
        let bytes = bytes.into();
        if bytes.is_empty() || bytes.len() > MAXIMUM_SOURCE_IMAGE_BYTES {
            return Err(Error::Configuration("source image is empty or too large"));
        }
        Ok(Self {
            filename: format!("img2img-{}.{}", Uuid::new_v4(), extension_for_mime(&mime)),
            bytes,
            mime,
        })
    }

    /// Creates an image source when only its encoded contents are available.
    pub fn from_bytes(bytes: impl Into<bytes::Bytes>) -> Result<Self, Error> {
        let bytes = bytes.into();
        let mime = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            "image/png"
        } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
            "image/jpeg"
        } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
            "image/webp"
        } else {
            return Err(Error::Configuration("source image type is not supported"));
        };
        Self::new(bytes, mime)
    }
}

fn normalize_source_mime(mime: &str) -> Result<String, Error> {
    match mime.trim().to_ascii_lowercase().as_str() {
        "image/jpg" | "image/jpeg" => Ok("image/jpeg".to_string()),
        "image/png" => Ok("image/png".to_string()),
        "image/webp" => Ok("image/webp".to_string()),
        _ => Err(Error::Configuration("source image type is not supported")),
    }
}

fn extension_for_mime(mime: &str) -> &'static str {
    MediaType::for_mime(mime)
        .filter(MediaType::is_image)
        .unwrap_or(MediaType::PNG)
        .extension
}
