use crate::caption::data_url;
use std::fmt;

/// One image on its way to a caption.
#[derive(Clone)]
#[non_exhaustive]
pub struct Draft {
    /// Inline data URL, the only image shape a vision model takes.
    pub image: String,
    pub caption: String,
    /// Drafts sharing a group are described once and captioned alike.
    pub group: usize,
}

impl fmt::Debug for Draft {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Draft")
            .field("image", &self.image.len())
            .field("caption", &self.caption)
            .field("group", &self.group)
            .finish()
    }
}

impl Draft {
    /// A draft of the image `filename` from its base64, carrying `caption`,
    /// blank to have one written, in the shot `group`.
    pub fn new(filename: &str, base64: &str, caption: &str, group: usize) -> Self {
        Self {
            image: data_url(filename, base64),
            caption: caption.to_string(),
            group,
        }
    }
}
