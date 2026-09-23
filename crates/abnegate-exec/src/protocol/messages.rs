//! Protocol message types for communication between the backend and the Rust runner.
//!
//! All messages are serialized as newline-delimited JSON (NDJSON).

use crate::executor::{Confinement, ConfinementMode};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// Protocol version for compatibility checking.
///
/// Confinement was added without a bump: `RunStart.confinement` defaults to
/// absent and the capability list is the negotiated extension point, so a
/// client that predates it keeps working unchanged. `ConfinementRequest`
/// grows the same way — `process_tree` defaults to absent, which is the
/// single-command confinement that shipped first.
pub const PROTOCOL_VERSION: &str = "1.0";

/// Filesystem a confined job is allowed to see.
///
/// Everything outside these roots is denied, as is the network. Roots must be
/// absolute paths that exist when the job starts.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConfinementRequest {
    #[serde(default)]
    pub read_roots: Vec<PathBuf>,
    #[serde(default)]
    pub write_roots: Vec<PathBuf>,
    /// Absent asks for the single-command confinement: the job runs one
    /// executable and may neither fork nor exec anything else. Present asks
    /// for a bounded process tree, which a build tool needs and a verification
    /// recipe does not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process_tree: Option<ProcessTreeRequest>,
}

impl ConfinementRequest {
    /// How much of a process tree this request asks for.
    pub fn mode(&self) -> ConfinementMode {
        match self.process_tree {
            Some(_) => ConfinementMode::ProcessTree,
            None => ConfinementMode::SingleCommand,
        }
    }
}

/// The bound on a confined process tree.
///
/// A tree may only start executables that live under one of `execute_roots`,
/// plus the entry command itself. Read and write roots never confer the right
/// to execute, so a tree cannot write a binary into its workspace and then run
/// it unless the caller named that directory here.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProcessTreeRequest {
    #[serde(default)]
    pub execute_roots: Vec<PathBuf>,
}

/// Messages sent from the backend to the Runner
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum InboundMessage {
    /// Handshake message to establish connection
    Hello {
        protocol_version: String,
        #[serde(default)]
        capabilities: Vec<String>,
    },

    /// Start a new command execution
    RunStart {
        job_id: String,
        workspace: PathBuf,
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        env: HashMap<String, String>,
        #[serde(default)]
        timeout_ms: Option<u64>,
        #[serde(default)]
        max_output_bytes: Option<usize>,
        #[serde(default)]
        working_dir: Option<PathBuf>,
        /// When present the job runs under OS confinement, and fails to start
        /// if this runner cannot prove confinement works. Boxed because most
        /// jobs carry none and the request is the largest thing in the enum.
        #[serde(default)]
        confinement: Option<Box<ConfinementRequest>>,
    },

    /// Send data to a running command's stdin
    RunStdin {
        job_id: String,
        /// Base64 encoded data
        data: String,
        #[serde(default)]
        eof: bool,
    },

    /// Cancel a running command
    RunCancel {
        job_id: String,
        /// If true, use SIGKILL instead of SIGTERM
        #[serde(default)]
        force: bool,
    },

    /// Health check ping
    Ping { id: String },
}

/// Messages sent from the Runner to the backend
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum OutboundMessage {
    /// Response to Hello message
    HelloAck {
        protocol_version: String,
        runner_version: String,
        capabilities: Vec<String>,
    },

    /// Command has started executing
    RunStarted { job_id: String, pid: u32 },

    /// Chunk of stdout output
    RunStdout {
        job_id: String,
        /// Base64 encoded data
        data: String,
        sequence: u64,
    },

    /// Chunk of stderr output
    RunStderr {
        job_id: String,
        /// Base64 encoded data
        data: String,
        sequence: u64,
    },

    /// Structured log message from the runner
    RunLog {
        job_id: String,
        level: LogLevel,
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        details: Option<serde_json::Value>,
    },

    /// Command has exited normally
    RunExit {
        job_id: String,
        exit_code: Option<i32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        signal: Option<i32>,
        duration_ms: u64,
    },

    /// Command encountered an error
    RunError {
        job_id: String,
        error_code: ErrorCode,
        message: String,
    },

    /// Response to Ping message
    Pong { id: String },
}

/// Log levels for structured logging
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
}

/// Error codes for structured error reporting
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// Protocol-level error (invalid message format)
    InvalidMessage,
    /// Job ID not found
    JobNotFound,
    /// Failed to spawn the process
    SpawnFailed,
    /// Command timed out
    Timeout,
    /// Output limit exceeded (output was truncated)
    OutputLimitExceeded,
    /// Job was cancelled
    Cancelled,
    /// Internal runner error
    InternalError,
    /// Workspace path is invalid
    InvalidWorkspace,
    /// Confinement was requested but this runner cannot prove it works
    ConfinementUnavailable,
}

/// Runner capabilities advertised during handshake
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capability {
    /// Can cancel running jobs
    Cancel,
    /// Can send stdin to running jobs
    Stdin,
    /// Emits structured log messages
    Logs,
    /// Uses process groups for clean kill
    ProcessGroup,
    /// Can run jobs under OS-level confinement
    Confinement,
    /// Can confine a whole process tree, not just a single executable
    ConfinementProcessTree,
}

impl Capability {
    const ALL: [Capability; 6] = [
        Capability::Cancel,
        Capability::Stdin,
        Capability::Logs,
        Capability::ProcessGroup,
        Capability::Confinement,
        Capability::ConfinementProcessTree,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            Capability::Cancel => "cancel",
            Capability::Stdin => "stdin",
            Capability::Logs => "logs",
            Capability::ProcessGroup => "process_group",
            Capability::Confinement => "confinement",
            Capability::ConfinementProcessTree => "confinement_process_tree",
        }
    }

    /// Every capability the protocol defines.
    pub fn all() -> Vec<String> {
        Self::ALL
            .iter()
            .map(|capability| capability.as_str().to_string())
            .collect()
    }

    /// The capabilities this host can honour.
    pub fn supported() -> Vec<String> {
        Self::ALL
            .iter()
            .filter(|capability| capability.is_supported())
            .map(|capability| capability.as_str().to_string())
            .collect()
    }

    fn is_supported(&self) -> bool {
        match self {
            Capability::Confinement | Capability::ConfinementProcessTree => {
                Confinement::is_available()
            }
            _ => true,
        }
    }
}

impl OutboundMessage {
    /// Create a HelloAck with the capabilities this host can honour
    pub fn hello_ack() -> Self {
        OutboundMessage::HelloAck {
            protocol_version: PROTOCOL_VERSION.to_string(),
            runner_version: env!("CARGO_PKG_VERSION").to_string(),
            capabilities: Capability::supported(),
        }
    }

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
    fn test_hello_serialization() {
        let msg = InboundMessage::Hello {
            protocol_version: "1.0".to_string(),
            capabilities: vec!["cancel".to_string()],
        };

        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains(r#""type":"Hello""#));
        assert!(json.contains(r#""protocol_version":"1.0""#));
    }

    #[test]
    fn test_hello_deserialization_with_empty_capabilities() {
        let json = r#"{"type": "Hello", "protocol_version": "1.0", "capabilities": []}"#;
        let msg: InboundMessage = serde_json::from_str(json).unwrap();

        match msg {
            InboundMessage::Hello {
                protocol_version,
                capabilities,
            } => {
                assert_eq!(protocol_version, "1.0");
                assert!(capabilities.is_empty());
            }
            _ => panic!("Wrong message type"),
        }
    }

    #[test]
    fn test_hello_deserialization_without_capabilities() {
        let json = r#"{"type": "Hello", "protocol_version": "2.0"}"#;
        let msg: InboundMessage = serde_json::from_str(json).unwrap();

        match msg {
            InboundMessage::Hello {
                protocol_version,
                capabilities,
            } => {
                assert_eq!(protocol_version, "2.0");
                assert!(capabilities.is_empty());
            }
            _ => panic!("Wrong message type"),
        }
    }

    #[test]
    fn test_hello_deserialization_with_all_capabilities() {
        let json = r#"{"type": "Hello", "protocol_version": "1.0", "capabilities": ["cancel", "stdin", "logs", "process_group"]}"#;
        let msg: InboundMessage = serde_json::from_str(json).unwrap();

        match msg {
            InboundMessage::Hello { capabilities, .. } => {
                assert_eq!(capabilities.len(), 4);
                assert!(capabilities.contains(&"cancel".to_string()));
                assert!(capabilities.contains(&"stdin".to_string()));
            }
            _ => panic!("Wrong message type"),
        }
    }

    #[test]
    fn test_hello_ack_serialization() {
        let msg = OutboundMessage::hello_ack();
        let json = serde_json::to_string(&msg).unwrap();

        assert!(json.contains(r#""type":"HelloAck""#));
        assert!(json.contains(r#""protocol_version":"1.0""#));
        assert!(json.contains(r#""cancel""#));
        assert!(json.contains(r#""process_group""#));
    }

    #[test]
    fn test_hello_ack_roundtrip() {
        let original = OutboundMessage::hello_ack();
        let json = serde_json::to_string(&original).unwrap();
        let decoded: OutboundMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(original, decoded);
    }

    #[test]
    fn test_run_start_deserialization() {
        let json = r#"{
            "type": "RunStart",
            "job_id": "job-123",
            "workspace": "/tmp/work",
            "command": "echo",
            "args": ["hello", "world"],
            "env": {"FOO": "bar"}
        }"#;

        let msg: InboundMessage = serde_json::from_str(json).unwrap();
        match msg {
            InboundMessage::RunStart {
                job_id,
                command,
                args,
                env,
                ..
            } => {
                assert_eq!(job_id, "job-123");
                assert_eq!(command, "echo");
                assert_eq!(args, vec!["hello", "world"]);
                assert_eq!(env.get("FOO"), Some(&"bar".to_string()));
            }
            _ => panic!("Wrong message type"),
        }
    }

    #[test]
    fn test_run_start_minimal() {
        let json = r#"{
            "type": "RunStart",
            "job_id": "job-456",
            "workspace": "/tmp",
            "command": "ls"
        }"#;

        let msg: InboundMessage = serde_json::from_str(json).unwrap();
        match msg {
            InboundMessage::RunStart {
                job_id,
                args,
                env,
                timeout_ms,
                max_output_bytes,
                working_dir,
                ..
            } => {
                assert_eq!(job_id, "job-456");
                assert!(args.is_empty());
                assert!(env.is_empty());
                assert!(timeout_ms.is_none());
                assert!(max_output_bytes.is_none());
                assert!(working_dir.is_none());
            }
            _ => panic!("Wrong message type"),
        }
    }

    #[test]
    fn test_run_start_with_all_fields() {
        let json = r#"{
            "type": "RunStart",
            "job_id": "full-job",
            "workspace": "/home/user/project",
            "command": "bun",
            "args": ["install"],
            "env": {"NODE_ENV": "production", "CI": "true"},
            "timeout_ms": 300000,
            "max_output_bytes": 10485760,
            "working_dir": "/home/user/project/packages/app",
            "confinement": {
                "read_roots": ["/home/user/project"],
                "write_roots": ["/home/user/project/target"]
            }
        }"#;

        let msg: InboundMessage = serde_json::from_str(json).unwrap();
        match msg {
            InboundMessage::RunStart {
                job_id,
                workspace,
                command,
                args,
                env,
                timeout_ms,
                max_output_bytes,
                working_dir,
                confinement,
            } => {
                assert_eq!(job_id, "full-job");
                assert_eq!(workspace.to_str().unwrap(), "/home/user/project");
                assert_eq!(command, "bun");
                assert_eq!(args, vec!["install"]);
                assert_eq!(env.len(), 2);
                assert_eq!(env.get("NODE_ENV"), Some(&"production".to_string()));
                assert_eq!(env.get("CI"), Some(&"true".to_string()));
                assert_eq!(timeout_ms, Some(300000));
                assert_eq!(max_output_bytes, Some(10485760));
                assert_eq!(
                    working_dir.unwrap().to_str().unwrap(),
                    "/home/user/project/packages/app"
                );
                let confinement = confinement.unwrap();
                assert_eq!(
                    confinement.read_roots,
                    vec![PathBuf::from("/home/user/project")]
                );
                assert_eq!(
                    confinement.write_roots,
                    vec![PathBuf::from("/home/user/project/target")]
                );
            }
            _ => panic!("Wrong message type"),
        }
    }

    #[test]
    fn test_run_start_with_unicode_args() {
        let json = r#"{
            "type": "RunStart",
            "job_id": "unicode-job",
            "workspace": "/tmp",
            "command": "echo",
            "args": ["你好", "мир", "🌍"]
        }"#;

        let msg: InboundMessage = serde_json::from_str(json).unwrap();
        match msg {
            InboundMessage::RunStart { args, .. } => {
                assert_eq!(args, vec!["你好", "мир", "🌍"]);
            }
            _ => panic!("Wrong message type"),
        }
    }

    #[test]
    fn test_run_start_with_special_chars_in_env() {
        let json = r#"{
            "type": "RunStart",
            "job_id": "special-env",
            "workspace": "/tmp",
            "command": "bash",
            "env": {"PATH": "/usr/bin:/usr/local/bin", "MSG": "hello=world&foo=bar"}
        }"#;

        let msg: InboundMessage = serde_json::from_str(json).unwrap();
        match msg {
            InboundMessage::RunStart { env, .. } => {
                assert_eq!(
                    env.get("PATH"),
                    Some(&"/usr/bin:/usr/local/bin".to_string())
                );
                assert_eq!(env.get("MSG"), Some(&"hello=world&foo=bar".to_string()));
            }
            _ => panic!("Wrong message type"),
        }
    }

    #[test]
    fn test_run_stdin_deserialization() {
        let json = r#"{"type": "RunStdin", "job_id": "job-123", "data": "SGVsbG8gV29ybGQK", "eof": false}"#;
        let msg: InboundMessage = serde_json::from_str(json).unwrap();

        match msg {
            InboundMessage::RunStdin { job_id, data, eof } => {
                assert_eq!(job_id, "job-123");
                assert_eq!(data, "SGVsbG8gV29ybGQK");
                assert!(!eof);
            }
            _ => panic!("Wrong message type"),
        }
    }

    #[test]
    fn test_run_stdin_with_eof() {
        let json = r#"{"type": "RunStdin", "job_id": "job-123", "data": "", "eof": true}"#;
        let msg: InboundMessage = serde_json::from_str(json).unwrap();

        match msg {
            InboundMessage::RunStdin { eof, .. } => {
                assert!(eof);
            }
            _ => panic!("Wrong message type"),
        }
    }

    #[test]
    fn test_run_stdin_default_eof() {
        let json = r#"{"type": "RunStdin", "job_id": "job-123", "data": "dGVzdA=="}"#;
        let msg: InboundMessage = serde_json::from_str(json).unwrap();

        match msg {
            InboundMessage::RunStdin { eof, .. } => {
                assert!(!eof);
            }
            _ => panic!("Wrong message type"),
        }
    }

    #[test]
    fn test_run_cancel_deserialization() {
        let json = r#"{"type": "RunCancel", "job_id": "job-123", "force": true}"#;
        let msg: InboundMessage = serde_json::from_str(json).unwrap();

        match msg {
            InboundMessage::RunCancel { job_id, force } => {
                assert_eq!(job_id, "job-123");
                assert!(force);
            }
            _ => panic!("Wrong message type"),
        }
    }

    #[test]
    fn test_run_cancel_default_force() {
        let json = r#"{"type": "RunCancel", "job_id": "job-123"}"#;
        let msg: InboundMessage = serde_json::from_str(json).unwrap();

        match msg {
            InboundMessage::RunCancel { force, .. } => {
                assert!(!force);
            }
            _ => panic!("Wrong message type"),
        }
    }

    #[test]
    fn test_run_cancel_non_force() {
        let json = r#"{"type": "RunCancel", "job_id": "job-789", "force": false}"#;
        let msg: InboundMessage = serde_json::from_str(json).unwrap();

        match msg {
            InboundMessage::RunCancel { job_id, force } => {
                assert_eq!(job_id, "job-789");
                assert!(!force);
            }
            _ => panic!("Wrong message type"),
        }
    }

    #[test]
    fn test_ping_pong() {
        let ping = InboundMessage::Ping {
            id: "ping-1".to_string(),
        };
        let pong = OutboundMessage::Pong {
            id: "ping-1".to_string(),
        };

        let ping_json = serde_json::to_string(&ping).unwrap();
        let pong_json = serde_json::to_string(&pong).unwrap();

        assert!(ping_json.contains(r#""type":"Ping""#));
        assert!(pong_json.contains(r#""type":"Pong""#));
    }

    #[test]
    fn test_ping_roundtrip() {
        let original = InboundMessage::Ping {
            id: "test-ping-123".to_string(),
        };
        let json = serde_json::to_string(&original).unwrap();
        let decoded: InboundMessage = serde_json::from_str(&json).unwrap();
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
        let msg = OutboundMessage::RunStarted {
            job_id: "job-123".to_string(),
            pid: 12345,
        };

        let json = serde_json::to_string(&msg).unwrap();
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
        let msg = OutboundMessage::RunStdout {
            job_id: "job-123".to_string(),
            data: "SGVsbG8gV29ybGQK".to_string(),
            sequence: 1,
        };

        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains(r#""type":"RunStdout""#));
        assert!(json.contains(r#""data":"SGVsbG8gV29ybGQK""#));
        assert!(json.contains(r#""sequence":1"#));
    }

    #[test]
    fn test_run_stderr_serialization() {
        let msg = OutboundMessage::RunStderr {
            job_id: "job-123".to_string(),
            data: "RXJyb3IhCg==".to_string(),
            sequence: 5,
        };

        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains(r#""type":"RunStderr""#));
        assert!(json.contains(r#""data":"RXJyb3IhCg==""#));
        assert!(json.contains(r#""sequence":5"#));
    }

    #[test]
    fn test_run_stdout_large_sequence() {
        let msg = OutboundMessage::RunStdout {
            job_id: "job-123".to_string(),
            data: "dGVzdA==".to_string(),
            sequence: u64::MAX,
        };

        let json = serde_json::to_string(&msg).unwrap();
        let decoded: OutboundMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(msg, decoded);
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
            let msg = OutboundMessage::log("job-1", level, "test message", None);
            let json = serde_json::to_string(&msg).unwrap();
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

        let msg = OutboundMessage::log(
            "job-1",
            LogLevel::Warn,
            "Output truncated",
            Some(details.clone()),
        );

        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains(r#""bytes_written":10485760"#));
        assert!(json.contains(r#""truncated":true"#));
    }

    #[test]
    fn test_run_log_without_details() {
        let msg = OutboundMessage::log("job-1", LogLevel::Info, "Simple log", None);
        let json = serde_json::to_string(&msg).unwrap();

        assert!(!json.contains("details"));
    }

    #[test]
    fn test_run_exit_serialization() {
        let msg = OutboundMessage::RunExit {
            job_id: "job-123".to_string(),
            exit_code: Some(0),
            signal: None,
            duration_ms: 1500,
        };

        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains(r#""type":"RunExit""#));
        assert!(json.contains(r#""exit_code":0"#));
        assert!(json.contains(r#""duration_ms":1500"#));
        assert!(!json.contains("signal"));
    }

    #[test]
    fn test_run_exit_with_signal() {
        let msg = OutboundMessage::RunExit {
            job_id: "job-killed".to_string(),
            exit_code: None,
            signal: Some(9), // SIGKILL
            duration_ms: 5000,
        };

        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains(r#""signal":9"#));
        assert!(json.contains(r#""exit_code":null"#));
    }

    #[test]
    fn test_run_exit_non_zero() {
        let msg = OutboundMessage::RunExit {
            job_id: "job-failed".to_string(),
            exit_code: Some(1),
            signal: None,
            duration_ms: 100,
        };

        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains(r#""exit_code":1"#));
    }

    #[test]
    fn test_run_exit_negative_exit_code() {
        let msg = OutboundMessage::RunExit {
            job_id: "job-negative".to_string(),
            exit_code: Some(-1),
            signal: None,
            duration_ms: 50,
        };

        let json = serde_json::to_string(&msg).unwrap();
        let decoded: OutboundMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(msg, decoded);
    }

    #[test]
    fn test_run_error_serialization() {
        let msg =
            OutboundMessage::error("job-123", ErrorCode::Timeout, "Command timed out after 60s");

        let json = serde_json::to_string(&msg).unwrap();
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
            let msg = OutboundMessage::error("job-1", code, "test error");
            let json = serde_json::to_string(&msg).unwrap();
            assert!(
                json.contains(&format!(r#""error_code":"{}""#, expected_str)),
                "Expected {} in {}",
                expected_str,
                json
            );
        }
    }

    #[test]
    fn test_capability_as_str() {
        assert_eq!(Capability::Cancel.as_str(), "cancel");
        assert_eq!(Capability::Stdin.as_str(), "stdin");
        assert_eq!(Capability::Logs.as_str(), "logs");
        assert_eq!(Capability::ProcessGroup.as_str(), "process_group");
        assert_eq!(Capability::Confinement.as_str(), "confinement");
        assert_eq!(
            Capability::ConfinementProcessTree.as_str(),
            "confinement_process_tree"
        );
    }

    #[test]
    fn test_capability_all() {
        let all = Capability::all();
        assert_eq!(all.len(), 6);
        assert!(all.contains(&"cancel".to_string()));
        assert!(all.contains(&"stdin".to_string()));
        assert!(all.contains(&"logs".to_string()));
        assert!(all.contains(&"process_group".to_string()));
        assert!(all.contains(&"confinement".to_string()));
        assert!(all.contains(&"confinement_process_tree".to_string()));
    }

    #[test]
    fn test_capability_supported_tracks_confinement_availability() {
        let supported = Capability::supported();
        assert!(supported.contains(&"cancel".to_string()));
        assert_eq!(
            supported.contains(&"confinement".to_string()),
            Confinement::is_available()
        );
    }

    #[test]
    fn test_unknown_message_type() {
        let json = r#"{"type": "Unknown", "foo": "bar"}"#;
        let result: Result<InboundMessage, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }

    #[test]
    fn test_missing_required_field() {
        let json = r#"{"type": "RunStart", "workspace": "/tmp", "command": "ls"}"#;
        let result: Result<InboundMessage, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }

    #[test]
    fn test_invalid_json() {
        let json = r#"{"type": "Hello", "protocol_version": "#;
        let result: Result<InboundMessage, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }

    #[test]
    fn test_wrong_type_for_field() {
        let json = r#"{
            "type": "RunStart",
            "job_id": "job-1",
            "workspace": "/tmp",
            "command": "ls",
            "timeout_ms": "not a number"
        }"#;
        let result: Result<InboundMessage, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }

    #[test]
    fn test_protocol_version_constant() {
        assert_eq!(PROTOCOL_VERSION, "1.0");
    }

    #[test]
    fn test_empty_job_id() {
        let json = r#"{"type": "RunStart", "job_id": "", "workspace": "/tmp", "command": "ls"}"#;
        let msg: InboundMessage = serde_json::from_str(json).unwrap();

        match msg {
            InboundMessage::RunStart { job_id, .. } => {
                assert_eq!(job_id, "");
            }
            _ => panic!("Wrong message type"),
        }
    }

    #[test]
    fn test_very_long_job_id() {
        let long_id = "a".repeat(1000);
        let json = format!(
            r#"{{"type": "RunStart", "job_id": "{}", "workspace": "/tmp", "command": "ls"}}"#,
            long_id
        );
        let msg: InboundMessage = serde_json::from_str(&json).unwrap();

        match msg {
            InboundMessage::RunStart { job_id, .. } => {
                assert_eq!(job_id.len(), 1000);
            }
            _ => panic!("Wrong message type"),
        }
    }

    #[test]
    fn test_zero_timeout() {
        let json = r#"{
            "type": "RunStart",
            "job_id": "job-1",
            "workspace": "/tmp",
            "command": "ls",
            "timeout_ms": 0
        }"#;
        let msg: InboundMessage = serde_json::from_str(json).unwrap();

        match msg {
            InboundMessage::RunStart { timeout_ms, .. } => {
                assert_eq!(timeout_ms, Some(0));
            }
            _ => panic!("Wrong message type"),
        }
    }

    #[test]
    fn test_max_timeout() {
        let json = format!(
            r#"{{"type": "RunStart", "job_id": "job-1", "workspace": "/tmp", "command": "ls", "timeout_ms": {}}}"#,
            u64::MAX
        );
        let msg: InboundMessage = serde_json::from_str(&json).unwrap();

        match msg {
            InboundMessage::RunStart { timeout_ms, .. } => {
                assert_eq!(timeout_ms, Some(u64::MAX));
            }
            _ => panic!("Wrong message type"),
        }
    }
}
