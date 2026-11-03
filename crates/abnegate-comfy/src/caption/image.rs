use crate::caption::Draft;
use serde::Deserialize;
use std::fmt;

/// One image submitted for captioning, as base64 from a JSON body or as raw
/// bytes from a multipart upload. The raw bytes win when both are given.
#[derive(Deserialize)]
#[non_exhaustive]
pub struct CaptionImage {
    /// The image's name, whose extension picks its media type.
    pub filename: String,
    /// The encoded image, in standard base64.
    #[serde(default)]
    pub bytes_base64: String,
    /// The encoded image, as uploaded.
    #[serde(skip)]
    pub bytes: Option<Vec<u8>>,
    /// A caption a person already wrote, kept as it is; blank to have one
    /// written.
    #[serde(default)]
    pub caption: String,
    /// Images sharing a group show the same shot, so one description covers
    /// them all. Video frames arrive grouped; separate photos do not.
    #[serde(default)]
    pub group: Option<usize>,
}

impl CaptionImage {
    /// An image named `filename` from its base64 `bytes_base64`, with no
    /// caption yet and no shot of its own.
    pub fn new(filename: impl Into<String>, bytes_base64: impl Into<String>) -> Self {
        Self {
            filename: filename.into(),
            bytes_base64: bytes_base64.into(),
            bytes: None,
            caption: String::new(),
            group: None,
        }
    }

    /// An image named `filename` from its encoded `bytes` as uploaded, with
    /// no caption yet and no shot of its own.
    pub fn from_bytes(filename: impl Into<String>, bytes: impl Into<Vec<u8>>) -> Self {
        Self {
            bytes: Some(bytes.into()),
            ..Self::new(filename, String::new())
        }
    }

    /// Keeps `caption`, which a person wrote, instead of having one written.
    pub fn with_caption(mut self, caption: impl Into<String>) -> Self {
        self.caption = caption.into();
        self
    }

    /// Places the image in the shot `group`, so one description covers it and
    /// the rest of that shot.
    pub fn with_group(mut self, group: usize) -> Self {
        self.group = Some(group);
        self
    }

    /// The image on its way to a caption in the shot `group`, from the
    /// uploaded bytes when there are any and the base64 otherwise.
    pub fn draft(&self, group: usize) -> Draft {
        match self.bytes.as_deref().filter(|bytes| !bytes.is_empty()) {
            Some(bytes) => Draft::from_bytes(&self.filename, bytes, &self.caption, group),
            None => Draft::new(&self.filename, &self.bytes_base64, &self.caption, group),
        }
    }
}

impl fmt::Debug for CaptionImage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CaptionImage")
            .field("filename", &self.filename)
            .field("bytes_base64", &self.bytes_base64.len())
            .field("bytes", &self.bytes.as_ref().map(Vec::len))
            .field("caption", &self.caption)
            .field("group", &self.group)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_raw_bytes_leave_the_draft_to_the_base64() {
        let mut image = CaptionImage::new("one.png", "aGk=");
        image.bytes = Some(Vec::new());

        let draft = image.draft(0);

        assert_eq!(draft.image, Draft::new("one.png", "aGk=", "", 0).image);
    }
}
