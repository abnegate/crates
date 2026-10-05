use thiserror::Error;

/// Failure while reading a remote model catalogue.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum CatalogError {
    /// A request failed, or no HTTP client could be built, such as for an
    /// invalid proxy URL. The request URL is left out.
    #[error("HTTP request failed: {0}")]
    Http(reqwest::Error),
    /// The catalogue answered with something that could not be read.
    #[error("Failed to parse response: {0}")]
    Parse(String),
    /// The catalogue refused the request or is unknown, as the text says.
    #[error("Provider unavailable: {0}")]
    Unavailable(String),
    /// The catalogue answered with this failing HTTP status, such as `404`
    /// for a repository that does not exist.
    #[error("Catalogue returned HTTP status {0}")]
    Status(u16),
    /// Reading or writing a local file failed.
    #[error("Filesystem error: {0}")]
    Io(#[from] std::io::Error),
    /// A configured catalogue URL does not parse, or is not `http` or
    /// `https`.
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
        assert_eq!(
            CatalogError::Status(404).to_string(),
            "Catalogue returned HTTP status 404"
        );
    }
}
