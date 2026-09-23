use thiserror::Error;

/// Failure while reading a remote model catalogue.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum CatalogError {
    #[error("HTTP request failed: {0}")]
    Http(reqwest::Error),
    #[error("Failed to parse response: {0}")]
    Parse(String),
    #[error("Provider unavailable: {0}")]
    Unavailable(String),
    #[error("Filesystem error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Invalid catalogue URL: {0}")]
    InvalidUrl(String),
}

impl From<reqwest::Error> for CatalogError {
    fn from(error: reqwest::Error) -> Self {
        Self::Http(error.without_url())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_describe_themselves() {
        assert_eq!(
            CatalogError::Parse("Invalid JSON".to_string()).to_string(),
            "Failed to parse response: Invalid JSON"
        );
        assert_eq!(
            CatalogError::Unavailable("Service down".to_string()).to_string(),
            "Provider unavailable: Service down"
        );
    }
}
