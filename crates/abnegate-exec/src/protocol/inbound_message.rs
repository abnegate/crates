use std::collections::BTreeSet;
use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;

use serde::Deserialize;
use serde::Serialize;

use super::confinement_request::ConfinementRequest;

/// Messages a client sends to a runner
///
/// `Debug` prints the names in a `RunStart` environment and the length of a
/// `RunStdin` payload, never the values themselves: either can carry a
/// credential.
#[derive(Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
#[non_exhaustive]
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
        /// Variables layered over the runner's
        /// [`EnvironmentPolicy`](crate::executor::EnvironmentPolicy)
        #[serde(default)]
        env: HashMap<String, String>,
        #[serde(default)]
        timeout_ms: Option<u64>,
        /// Ceiling on stdout and stderr together; absent uses the
        /// executor's default
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

impl fmt::Debug for InboundMessage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InboundMessage::Hello {
                protocol_version,
                capabilities,
            } => formatter
                .debug_struct("Hello")
                .field("protocol_version", protocol_version)
                .field("capabilities", capabilities)
                .finish(),
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
            } => formatter
                .debug_struct("RunStart")
                .field("job_id", job_id)
                .field("workspace", workspace)
                .field("command", command)
                .field("args", args)
                .field("env", &env.keys().collect::<BTreeSet<&String>>())
                .field("timeout_ms", timeout_ms)
                .field("max_output_bytes", max_output_bytes)
                .field("working_dir", working_dir)
                .field("confinement", confinement)
                .finish(),
            InboundMessage::RunStdin { job_id, data, eof } => formatter
                .debug_struct("RunStdin")
                .field("job_id", job_id)
                .field("data", &format_args!("<{} bytes>", data.len()))
                .field("eof", eof)
                .finish(),
            InboundMessage::RunCancel { job_id, force } => formatter
                .debug_struct("RunCancel")
                .field("job_id", job_id)
                .field("force", force)
                .finish(),
            InboundMessage::Ping { id } => formatter.debug_struct("Ping").field("id", id).finish(),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::protocol::OutboundMessage;

    use super::*;

    #[test]
    fn debug_never_prints_an_environment_value_or_stdin() {
        let run = InboundMessage::RunStart {
            job_id: "job".to_string(),
            workspace: PathBuf::from("/tmp"),
            command: "env".to_string(),
            args: vec![],
            env: HashMap::from([("APP_MASTER_KEY".to_string(), "hunter2".to_string())]),
            timeout_ms: None,
            max_output_bytes: None,
            working_dir: None,
            confinement: None,
        };
        let stdin = InboundMessage::RunStdin {
            job_id: "job".to_string(),
            data: "aHVudGVyMgo=".to_string(),
            eof: false,
        };

        let run = format!("{run:?}");
        let stdin = format!("{stdin:?}");

        assert!(run.contains("APP_MASTER_KEY"), "{run}");
        assert!(!run.contains("hunter2"), "{run}");
        assert!(stdin.contains("<12 bytes>"), "{stdin}");
        assert!(!stdin.contains("aHVudGVyMgo="), "{stdin}");
    }

    #[test]
    fn test_hello_serialization() {
        let message = InboundMessage::Hello {
            protocol_version: "1.0".to_string(),
            capabilities: vec!["cancel".to_string()],
        };

        let json = serde_json::to_string(&message).unwrap();
        assert!(json.contains(r#""type":"Hello""#));
        assert!(json.contains(r#""protocol_version":"1.0""#));
    }

    #[test]
    fn test_hello_deserialization_with_empty_capabilities() {
        let json = r#"{"type": "Hello", "protocol_version": "1.0", "capabilities": []}"#;
        let message: InboundMessage = serde_json::from_str(json).unwrap();

        match message {
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
        let message: InboundMessage = serde_json::from_str(json).unwrap();

        match message {
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
        let message: InboundMessage = serde_json::from_str(json).unwrap();

        match message {
            InboundMessage::Hello { capabilities, .. } => {
                assert_eq!(capabilities.len(), 4);
                assert!(capabilities.contains(&"cancel".to_string()));
                assert!(capabilities.contains(&"stdin".to_string()));
            }
            _ => panic!("Wrong message type"),
        }
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

        let message: InboundMessage = serde_json::from_str(json).unwrap();
        match message {
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

        let message: InboundMessage = serde_json::from_str(json).unwrap();
        match message {
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

        let message: InboundMessage = serde_json::from_str(json).unwrap();
        match message {
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

        let message: InboundMessage = serde_json::from_str(json).unwrap();
        match message {
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

        let message: InboundMessage = serde_json::from_str(json).unwrap();
        match message {
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
        let message: InboundMessage = serde_json::from_str(json).unwrap();

        match message {
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
        let message: InboundMessage = serde_json::from_str(json).unwrap();

        match message {
            InboundMessage::RunStdin { eof, .. } => {
                assert!(eof);
            }
            _ => panic!("Wrong message type"),
        }
    }

    #[test]
    fn test_run_stdin_default_eof() {
        let json = r#"{"type": "RunStdin", "job_id": "job-123", "data": "dGVzdA=="}"#;
        let message: InboundMessage = serde_json::from_str(json).unwrap();

        match message {
            InboundMessage::RunStdin { eof, .. } => {
                assert!(!eof);
            }
            _ => panic!("Wrong message type"),
        }
    }

    #[test]
    fn test_run_cancel_deserialization() {
        let json = r#"{"type": "RunCancel", "job_id": "job-123", "force": true}"#;
        let message: InboundMessage = serde_json::from_str(json).unwrap();

        match message {
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
        let message: InboundMessage = serde_json::from_str(json).unwrap();

        match message {
            InboundMessage::RunCancel { force, .. } => {
                assert!(!force);
            }
            _ => panic!("Wrong message type"),
        }
    }

    #[test]
    fn test_run_cancel_non_force() {
        let json = r#"{"type": "RunCancel", "job_id": "job-789", "force": false}"#;
        let message: InboundMessage = serde_json::from_str(json).unwrap();

        match message {
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
    fn test_empty_job_id() {
        let json = r#"{"type": "RunStart", "job_id": "", "workspace": "/tmp", "command": "ls"}"#;
        let message: InboundMessage = serde_json::from_str(json).unwrap();

        match message {
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
        let message: InboundMessage = serde_json::from_str(&json).unwrap();

        match message {
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
        let message: InboundMessage = serde_json::from_str(json).unwrap();

        match message {
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
        let message: InboundMessage = serde_json::from_str(&json).unwrap();

        match message {
            InboundMessage::RunStart { timeout_ms, .. } => {
                assert_eq!(timeout_ms, Some(u64::MAX));
            }
            _ => panic!("Wrong message type"),
        }
    }
}
