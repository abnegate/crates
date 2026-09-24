use std::fmt;

#[non_exhaustive]
pub struct GeneratedImage {
    pub bytes: bytes::Bytes,
    pub mime: String,
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
