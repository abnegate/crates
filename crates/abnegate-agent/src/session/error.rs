use thiserror::Error;
use uuid::Uuid;

/// Session storage error
#[derive(Debug, Error)]
pub enum SessionError {
    #[error("Session not found: {0}")]
    NotFound(Uuid),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_error_not_found_display() {
        let id = Uuid::new_v4();
        let error = SessionError::NotFound(id);
        let display = format!("{}", error);

        assert!(display.contains("Session not found"));
        assert!(display.contains(&id.to_string()));
    }

    #[test]
    fn test_session_error_io_from() {
        let io_error = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let session_error: SessionError = io_error.into();
        let display = format!("{}", session_error);

        assert!(display.contains("IO error"));
    }

    #[test]
    fn test_session_error_serialization_from() {
        let json_error = serde_json::from_str::<serde_json::Value>("invalid").unwrap_err();
        let session_error: SessionError = json_error.into();
        let display = format!("{}", session_error);

        assert!(display.contains("Serialization error"));
    }

    #[test]
    fn test_session_error_debug() {
        let id = Uuid::new_v4();
        let error = SessionError::NotFound(id);
        let debug_str = format!("{:?}", error);

        assert!(debug_str.contains("NotFound"));
    }
}
