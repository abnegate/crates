use serde::Deserialize;
use std::fmt;

/// One image of a training set.
#[derive(Deserialize)]
#[non_exhaustive]
pub struct TrainImage {
    /// Name the image was uploaded under.
    pub filename: String,
    /// Its caption, or for an edit base the instruction that turns the
    /// reference into it. Blank asks the vision model for one.
    pub caption: String,
    /// The encoded image, in base64.
    pub bytes_base64: String,
    /// The reference an edit adapter learns to change into this image, in
    /// base64. Only edit bases take one.
    #[serde(default)]
    pub before_base64: Option<String>,
    /// Images sharing a group are the same shot and are captioned together.
    /// Frames pulled from a clip arrive grouped; separate photos do not.
    #[serde(default)]
    pub group: Option<usize>,
}

impl TrainImage {
    /// An image named `filename`, captioned `caption`, from its base64
    /// `bytes_base64`, with no reference and no shot of its own.
    pub fn new(
        filename: impl Into<String>,
        caption: impl Into<String>,
        bytes_base64: impl Into<String>,
    ) -> Self {
        Self {
            filename: filename.into(),
            caption: caption.into(),
            bytes_base64: bytes_base64.into(),
            before_base64: None,
            group: None,
        }
    }

    /// Sets the reference, in base64, that an edit adapter learns to change
    /// into this image.
    pub fn with_before_base64(mut self, before_base64: impl Into<String>) -> Self {
        self.before_base64 = Some(before_base64.into());
        self
    }

    /// Places the image in the shot `group`, so it is captioned with the rest
    /// of that shot.
    pub fn with_group(mut self, group: usize) -> Self {
        self.group = Some(group);
        self
    }
}

impl fmt::Debug for TrainImage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TrainImage")
            .field("filename", &self.filename)
            .field("caption", &self.caption)
            .field("bytes_base64", &self.bytes_base64.len())
            .field(
                "before_base64",
                &self.before_base64.as_ref().map(String::len),
            )
            .field("group", &self.group)
            .finish()
    }
}
