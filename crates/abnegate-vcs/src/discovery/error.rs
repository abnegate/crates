use thiserror::Error;

/// What went wrong reading a manifest.
#[derive(Debug, Error)]
pub enum DiscoveryError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Malformed manifest: {0}")]
    Malformed(#[from] serde_json::Error),
}

pub type DiscoveryResult<T> = Result<T, DiscoveryError>;
