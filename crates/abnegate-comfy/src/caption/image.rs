use serde::Deserialize;
use std::fmt;

#[derive(Deserialize)]
pub struct CaptionImage {
    pub filename: String,
    pub bytes_base64: String,
    #[serde(default)]
    pub caption: String,
    /// Images sharing a group show the same shot, so one description covers
    /// them all. Video frames arrive grouped; separate photos do not.
    #[serde(default)]
    pub group: Option<usize>,
}

impl fmt::Debug for CaptionImage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CaptionImage")
            .field("filename", &self.filename)
            .field("bytes_base64", &self.bytes_base64.len())
            .field("caption", &self.caption)
            .field("group", &self.group)
            .finish()
    }
}
