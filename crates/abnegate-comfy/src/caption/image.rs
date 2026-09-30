use serde::Deserialize;
use std::fmt;

/// One image submitted for captioning.
#[derive(Deserialize)]
#[non_exhaustive]
pub struct CaptionImage {
    /// The image's name, whose extension picks its media type.
    pub filename: String,
    /// The encoded image, in standard base64.
    pub bytes_base64: String,
    /// A caption a person already wrote, kept as it is; blank to have one
    /// written.
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
