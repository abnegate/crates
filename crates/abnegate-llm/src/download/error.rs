use thiserror::Error;

/// A download that could not be completed, or completed wrong.
///
/// A variant may gain a field in a minor release, so a pattern outside this
/// crate ends in `..`:
///
/// ```compile_fail,E0638
/// use abnegate_llm::download::DownloadError;
///
/// fn missing(error: &DownloadError) -> Option<u64> {
///     match error {
///         DownloadError::Incomplete { expected, received } => Some(expected - received),
///         _ => None,
///     }
/// }
/// # let _ = missing;
/// ```
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum DownloadError {
    /// The request failed, or no HTTP client could be built. The request
    /// URL is left out.
    #[error("HTTP request failed: {0}")]
    Http(reqwest::Error),
    /// The server answered with this failing HTTP status.
    #[error("download failed with status {0}")]
    Status(u16),
    /// Writing, locking or renaming the file failed.
    #[error("filesystem error: {0}")]
    Io(#[from] std::io::Error),
    /// The file holds `received` of the `expected` bytes.
    #[error("the download holds {received} of its {expected} bytes")]
    #[non_exhaustive]
    Incomplete {
        /// The file's full size, in bytes.
        expected: u64,
        /// The bytes that arrived.
        received: u64,
    },
    /// The file's SHA-256 is not the one the caller expected.
    #[error("the downloaded file's SHA-256 is {actual}, not the expected {expected}")]
    #[non_exhaustive]
    Checksum {
        /// The digest asked for, in lowercase hexadecimal.
        expected: String,
        /// The digest of what arrived, in lowercase hexadecimal.
        actual: String,
    },
    /// A digest given as text is not 64 hexadecimal characters.
    #[error("{0} is not a 64-character hexadecimal SHA-256 digest")]
    InvalidChecksum(String),
    /// The server's copy changed under a resumed download, and again after
    /// starting over.
    #[error("the download was still inconsistent after starting over")]
    Inconsistent,
    /// Another transfer, in this process or another, holds the target.
    #[error("another download to the same file is in progress")]
    InProgress,
}

impl From<reqwest::Error> for DownloadError {
    fn from(error: reqwest::Error) -> Self {
        Self::Http(error.without_url())
    }
}
