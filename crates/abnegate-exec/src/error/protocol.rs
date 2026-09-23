use std::io;

use thiserror::Error;

/// Errors that can occur during protocol communication.
#[derive(Debug, Error)]
pub enum ProtocolError {
    /// Failed to parse JSON message
    #[error("Failed to parse JSON: {source} (line: {line})")]
    JsonParse {
        source: serde_json::Error,
        line: String,
    },

    /// Failed to serialize JSON message
    #[error("Failed to serialize JSON: {0}")]
    JsonSerialize(#[source] serde_json::Error),

    /// Line exceeds maximum allowed length
    #[error("Line too long: {length} bytes (max: {max})")]
    LineTooLong { length: usize, max: usize },

    /// I/O error during communication
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_protocol_error_json_parse() {
        let json_err = serde_json::from_str::<serde_json::Value>("invalid").unwrap_err();
        let err = ProtocolError::JsonParse {
            source: json_err,
            line: "invalid".to_string(),
        };
        assert!(err.to_string().contains("Failed to parse JSON"));
        assert!(err.to_string().contains("invalid"));
    }

    #[test]
    fn test_protocol_error_json_serialize() {
        struct BadSerializer;
        impl serde::Serialize for BadSerializer {
            fn serialize<S>(&self, _serializer: S) -> Result<S::Ok, S::Error>
            where
                S: serde::Serializer,
            {
                Err(serde::ser::Error::custom("intentional serialization error"))
            }
        }

        let json_err = serde_json::to_string(&BadSerializer).unwrap_err();
        let err = ProtocolError::JsonSerialize(json_err);
        assert!(err.to_string().contains("Failed to serialize JSON"));
    }

    #[test]
    fn test_protocol_error_line_too_long() {
        let err = ProtocolError::LineTooLong {
            length: 2000,
            max: 1000,
        };
        assert!(err.to_string().contains("2000"));
        assert!(err.to_string().contains("1000"));
    }

    #[test]
    fn test_protocol_error_io() {
        let io_err = io::Error::new(io::ErrorKind::UnexpectedEof, "unexpected EOF");
        let err = ProtocolError::Io(io_err);
        assert!(err.to_string().contains("I/O error"));
    }

    #[test]
    fn test_protocol_error_from_io() {
        let io_err = io::Error::new(io::ErrorKind::ConnectionReset, "reset");
        let err: ProtocolError = io_err.into();
        match err {
            ProtocolError::Io(_) => {}
            _ => panic!("Expected Io variant"),
        }
    }
}
