use serde::Deserialize;
use std::fmt;

#[derive(Deserialize)]
pub struct TrainImage {
    pub filename: String,
    pub caption: String,
    pub bytes_base64: String,
    #[serde(default)]
    pub before_base64: Option<String>,
    /// Images sharing a group are the same shot and are captioned together.
    /// Frames pulled from a clip arrive grouped; separate photos do not.
    #[serde(default)]
    pub group: Option<usize>,
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
