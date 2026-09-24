use std::time::Duration;

use serde::Deserialize;
use serde::Serialize;

use super::error_code::ErrorCode;
use super::log_level::LogLevel;
use super::milliseconds;

/// Messages a runner sends to its client
///
/// Each variant is `#[non_exhaustive]`: match it with `..`, since a field
/// can be added to any of them.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
#[non_exhaustive]
pub enum OutboundMessage {
    /// Response to Hello message, tagged `HelloAck` on the wire
    #[serde(rename = "HelloAck")]
    #[non_exhaustive]
    HelloAcknowledged {
        /// The protocol version the runner speaks
        protocol_version: String,
        /// The version of this crate the runner was built from
        runner_version: String,
        /// The capabilities the runner honours, by their
        /// [handshake names](super::Capability::as_str)
        capabilities: Vec<String>,
    },

    /// Command has started executing
    #[non_exhaustive]
    RunStarted { job_id: String, pid: u32 },

    /// Chunk of stdout output
    #[non_exhaustive]
    RunStdout {
        job_id: String,
        /// Base64 encoded data
        data: String,
        sequence: u64,
    },

    /// Chunk of stderr output
    #[non_exhaustive]
    RunStderr {
        job_id: String,
        /// Base64 encoded data
        data: String,
        sequence: u64,
    },

    /// Structured log message from the runner
    #[non_exhaustive]
    RunLog {
        job_id: String,
        level: LogLevel,
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        details: Option<serde_json::Value>,
    },

    /// Command has exited normally
    #[non_exhaustive]
    RunExit {
        job_id: String,
        exit_code: Option<i32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        signal: Option<i32>,
        /// How long the command ran, from its spawn until its group was
        /// gone. Whole milliseconds on the wire.
        #[serde(rename = "duration_ms", with = "milliseconds")]
        duration: Duration,
    },

    /// Command encountered an error
    #[non_exhaustive]
    RunError {
        job_id: String,
        error_code: ErrorCode,
        message: String,
    },

    /// Response to Ping message
    #[non_exhaustive]
    Pong { id: String },
}

impl OutboundMessage {
    /// Create a RunError message
    pub fn error(job_id: impl Into<String>, code: ErrorCode, message: impl Into<String>) -> Self {
        OutboundMessage::RunError {
            job_id: job_id.into(),
            error_code: code,
            message: message.into(),
        }
    }

    /// Create a RunLog message
    pub fn log(
        job_id: impl Into<String>,
        level: LogLevel,
        message: impl Into<String>,
        details: Option<serde_json::Value>,
    ) -> Self {
        OutboundMessage::RunLog {
            job_id: job_id.into(),
            level,
            message: message.into(),
            details,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hello_acknowledged_serialization() {
        let message = OutboundMessage::hello_acknowledged();
        let json = serde_json::to_string(&message).unwrap();

        assert!(json.contains(r#""type":"HelloAck""#));
        assert!(json.contains(r#""protocol_version":"1.0""#));
        assert!(json.contains(r#""cancel""#));
        assert!(json.contains(r#""process_group""#));
    }

    #[test]
    fn test_hello_acknowledged_roundtrip() {
        let original = OutboundMessage::hello_acknowledged();
        let json = serde_json::to_string(&original).unwrap();
        let decoded: OutboundMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(original, decoded);
    }

    #[test]
    fn test_pong_roundtrip() {
        let original = OutboundMessage::Pong {
            id: "test-pong-456".to_string(),
        };
        let json = serde_json::to_string(&original).unwrap();
        let decoded: OutboundMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(original, decoded);
    }

    #[test]
    fn test_run_started_serialization() {
        let message = OutboundMessage::RunStarted {
            job_id: "job-123".to_string(),
            pid: 12345,
        };

        let json = serde_json::to_string(&message).unwrap();
        assert!(json.contains(r#""type":"RunStarted""#));
        assert!(json.contains(r#""job_id":"job-123""#));
        assert!(json.contains(r#""pid":12345"#));
    }

    #[test]
    fn test_run_started_roundtrip() {
        let original = OutboundMessage::RunStarted {
            job_id: "roundtrip-job".to_string(),
            pid: 99999,
        };
        let json = serde_json::to_string(&original).unwrap();
        let decoded: OutboundMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(original, decoded);
    }

    #[test]
    fn test_run_stdout_serialization() {
        let message = OutboundMessage::RunStdout {
            job_id: "job-123".to_string(),
            data: "SGVsbG8gV29ybGQK".to_string(),
            sequence: 1,
        };

        let json = serde_json::to_string(&message).unwrap();
        assert!(json.contains(r#""type":"RunStdout""#));
        assert!(json.contains(r#""data":"SGVsbG8gV29ybGQK""#));
        assert!(json.contains(r#""sequence":1"#));
    }

    #[test]
    fn test_run_stderr_serialization() {
        let message = OutboundMessage::RunStderr {
            job_id: "job-123".to_string(),
            data: "RXJyb3IhCg==".to_string(),
            sequence: 5,
        };

        let json = serde_json::to_string(&message).unwrap();
        assert!(json.contains(r#""type":"RunStderr""#));
        assert!(json.contains(r#""data":"RXJyb3IhCg==""#));
        assert!(json.contains(r#""sequence":5"#));
    }

    #[test]
    fn test_run_stdout_large_sequence() {
        let message = OutboundMessage::RunStdout {
            job_id: "job-123".to_string(),
            data: "dGVzdA==".to_string(),
            sequence: u64::MAX,
        };

        let json = serde_json::to_string(&message).unwrap();
        let decoded: OutboundMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(message, decoded);
    }

    #[test]
    fn test_run_log_all_levels() {
        let levels = vec![
            (LogLevel::Debug, "debug"),
            (LogLevel::Info, "info"),
            (LogLevel::Warn, "warn"),
            (LogLevel::Error, "error"),
        ];

        for (level, expected_str) in levels {
            let message = OutboundMessage::log("job-1", level, "test message", None);
            let json = serde_json::to_string(&message).unwrap();
            assert!(json.contains(&format!(r#""level":"{}""#, expected_str)));
        }
    }

    #[test]
    fn test_run_log_with_details() {
        let details = serde_json::json!({
            "bytes_written": 10485760,
            "limit": 10485760,
            "truncated": true
        });

        let message = OutboundMessage::log(
            "job-1",
            LogLevel::Warn,
            "Output truncated",
            Some(details.clone()),
        );

        let json = serde_json::to_string(&message).unwrap();
        assert!(json.contains(r#""bytes_written":10485760"#));
        assert!(json.contains(r#""truncated":true"#));
    }

    #[test]
    fn test_run_log_without_details() {
        let message = OutboundMessage::log("job-1", LogLevel::Info, "Simple log", None);
        let json = serde_json::to_string(&message).unwrap();

        assert!(!json.contains("details"));
    }

    #[test]
    fn test_run_exit_serialization() {
        let message = OutboundMessage::RunExit {
            job_id: "job-123".to_string(),
            exit_code: Some(0),
            signal: None,
            duration: Duration::from_millis(1500),
        };

        let json = serde_json::to_string(&message).unwrap();
        assert!(json.contains(r#""type":"RunExit""#));
        assert!(json.contains(r#""exit_code":0"#));
        assert!(json.contains(r#""duration_ms":1500"#));
        assert!(!json.contains("signal"));
    }

    #[test]
    fn test_run_exit_with_signal() {
        let message = OutboundMessage::RunExit {
            job_id: "job-killed".to_string(),
            exit_code: None,
            signal: Some(9), // SIGKILL
            duration: Duration::from_millis(5000),
        };

        let json = serde_json::to_string(&message).unwrap();
        assert!(json.contains(r#""signal":9"#));
        assert!(json.contains(r#""exit_code":null"#));
    }

    #[test]
    fn test_run_exit_non_zero() {
        let message = OutboundMessage::RunExit {
            job_id: "job-failed".to_string(),
            exit_code: Some(1),
            signal: None,
            duration: Duration::from_millis(100),
        };

        let json = serde_json::to_string(&message).unwrap();
        assert!(json.contains(r#""exit_code":1"#));
    }

    #[test]
    fn test_run_exit_negative_exit_code() {
        let message = OutboundMessage::RunExit {
            job_id: "job-negative".to_string(),
            exit_code: Some(-1),
            signal: None,
            duration: Duration::from_millis(50),
        };

        let json = serde_json::to_string(&message).unwrap();
        let decoded: OutboundMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(message, decoded);
    }

    #[test]
    fn test_run_error_serialization() {
        let message =
            OutboundMessage::error("job-123", ErrorCode::Timeout, "Command timed out after 60s");

        let json = serde_json::to_string(&message).unwrap();
        assert!(json.contains(r#""type":"RunError""#));
        assert!(json.contains(r#""error_code":"timeout""#));
    }

    #[test]
    fn test_all_error_codes() {
        let error_codes = vec![
            (ErrorCode::InvalidMessage, "invalid_message"),
            (ErrorCode::JobNotFound, "job_not_found"),
            (ErrorCode::SpawnFailed, "spawn_failed"),
            (ErrorCode::Timeout, "timeout"),
            (ErrorCode::OutputLimitExceeded, "output_limit_exceeded"),
            (ErrorCode::Cancelled, "cancelled"),
            (ErrorCode::InternalError, "internal_error"),
            (ErrorCode::InvalidWorkspace, "invalid_workspace"),
            (ErrorCode::ConfinementUnavailable, "confinement_unavailable"),
        ];

        for (code, expected_str) in error_codes {
            let message = OutboundMessage::error("job-1", code, "test error");
            let json = serde_json::to_string(&message).unwrap();
            assert!(
                json.contains(&format!(r#""error_code":"{}""#, expected_str)),
                "Expected {} in {}",
                expected_str,
                json
            );
        }
    }
}
