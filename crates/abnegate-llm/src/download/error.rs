use thiserror::Error;

/// A download that could not be completed, or completed wrong.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum DownloadError {
    #[error("HTTP request failed: {0}")]
    Http(reqwest::Error),
    #[error("download failed with status {0}")]
    Status(u16),
    #[error("filesystem error: {0}")]
    Io(#[from] std::io::Error),
    #[error("the server sent {received} of {expected} bytes")]
    Incomplete { expected: u64, received: u64 },
    #[error("the downloaded file's SHA-256 is {actual}, not the expected {expected}")]
    Checksum { expected: String, actual: String },
    #[error("{0} is not a 64-character hexadecimal SHA-256 digest")]
    InvalidChecksum(String),
    #[error("the server kept answering the resumed range inconsistently")]
    Inconsistent,
}

impl From<reqwest::Error> for DownloadError {
    fn from(error: reqwest::Error) -> Self {
        Self::Http(error.without_url())
    }
}
