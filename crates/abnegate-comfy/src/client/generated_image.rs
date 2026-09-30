use std::fmt;

/// One file a workflow produced: an image, a video or audio, despite the
/// name.
#[non_exhaustive]
pub struct GeneratedImage {
    /// The encoded file.
    pub bytes: bytes::Bytes,
    /// Its media type, as the server reported it or, failing that, as its
    /// filename implies.
    pub mime: String,
    /// The name ComfyUI saved it under.
    pub filename: String,
}

impl fmt::Debug for GeneratedImage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GeneratedImage")
            .field("bytes", &self.bytes.len())
            .field("mime", &self.mime)
            .field("filename", &self.filename)
            .finish()
    }
}
