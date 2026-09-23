use std::io;

use thiserror::Error;

/// Errors that can occur during protocol communication.
#[derive(Debug, Error)]
pub enum ProtocolError {
    /// A line was not a valid message. Only its length is kept: the line
    /// itself carries a job's environment and stdin.
    #[error("Failed to parse a {length}-byte JSON line")]
    JsonParse {
        #[source]
        source: serde_json::Error,
        length: usize,
    },

    /// Failed to serialize JSON message
    #[error("Failed to serialize JSON")]
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
    use std::error::Error as _;

    use super::*;

    #[test]
    fn test_protocol_error_json_parse() {
        let json_err = serde_json::from_str::<serde_json::Value>("invalid").unwrap_err();
        let err = ProtocolError::JsonParse {
            source: json_err,
            length: 7,
        };
        assert_eq!(err.to_string(), "Failed to parse a 7-byte JSON line");
    }

    #[test]
    fn a_parse_failure_names_its_cause_once() {
        let cause = serde_json::from_str::<serde_json::Value>("invalid").unwrap_err();
        let message = cause.to_string();
        let error = ProtocolError::JsonParse {
            source: cause,
            length: 7,
        };

        assert!(!error.to_string().contains(&message), "{error}");
        assert_eq!(
            error.source().map(ToString::to_string),
            Some(message),
            "the cause is reachable through the chain"
        );
    }

    #[test]
    fn a_serialize_failure_names_its_cause_once() {
        let cause = serde_json::from_str::<serde_json::Value>("invalid").unwrap_err();
        let message = cause.to_string();
        let error = ProtocolError::JsonSerialize(cause);

        assert_eq!(error.to_string(), "Failed to serialize JSON");
        assert_eq!(error.source().map(ToString::to_string), Some(message));
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
