/// What a provider call can fail with, whatever the modality.
#[derive(Debug, thiserror::Error)]
pub enum ModalityError {
    #[error("network error: {0}")]
    NetworkError(String),

    #[error("API error (status {status}): {message}")]
    ApiError { status: u16, message: String },

    #[error("parse error: {0}")]
    ParseError(String),

    #[error("configuration error: {0}")]
    ConfigError(String),

    #[error("unsupported operation: {0}")]
    Unsupported(String),

    #[error("IO error: {0}")]
    IoError(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_variant_says_what_went_wrong() {
        assert_eq!(
            ModalityError::NetworkError("connection refused".into()).to_string(),
            "network error: connection refused"
        );
        assert_eq!(
            ModalityError::ApiError {
                status: 429,
                message: "rate limited".into(),
            }
            .to_string(),
            "API error (status 429): rate limited"
        );
        assert_eq!(
            ModalityError::ParseError("invalid json".into()).to_string(),
            "parse error: invalid json"
        );
        assert_eq!(
            ModalityError::ConfigError("missing key".into()).to_string(),
            "configuration error: missing key"
        );
        assert_eq!(
            ModalityError::Unsupported("feature X".into()).to_string(),
            "unsupported operation: feature X"
        );
        assert_eq!(
            ModalityError::IoError("file not found".into()).to_string(),
            "IO error: file not found"
        );
    }
}
