use serde::Deserialize;
use serde::Serialize;

use super::hello::Hello;
use super::ping::Ping;
use super::run_cancel::RunCancel;
use super::run_start::RunStart;
use super::run_stdin::RunStdin;

/// Messages a client sends to a runner
///
/// On the wire each is one JSON object: the variant's name under `type`,
/// beside the fields of the message it carries.
///
/// `Debug` prints the names in a `RunStart` environment and the length of a
/// `RunStdin` payload, never the values themselves: either can carry a
/// credential.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type")]
#[non_exhaustive]
pub enum InboundMessage {
    /// Handshake message to establish connection
    Hello(Hello),

    /// Start a new command execution
    RunStart(RunStart),

    /// Send data to a running command's stdin
    RunStdin(RunStdin),

    /// Cancel a running command
    RunCancel(RunCancel),

    /// Health check ping
    Ping(Ping),
}

impl From<Hello> for InboundMessage {
    fn from(hello: Hello) -> Self {
        Self::Hello(hello)
    }
}

impl From<RunStart> for InboundMessage {
    fn from(run: RunStart) -> Self {
        Self::RunStart(run)
    }
}

impl From<RunStdin> for InboundMessage {
    fn from(stdin: RunStdin) -> Self {
        Self::RunStdin(stdin)
    }
}

impl From<RunCancel> for InboundMessage {
    fn from(cancel: RunCancel) -> Self {
        Self::RunCancel(cancel)
    }
}

impl From<Ping> for InboundMessage {
    fn from(ping: Ping) -> Self {
        Self::Ping(ping)
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::Duration;

    use crate::protocol::ConfinementRequest;
    use crate::protocol::OutboundMessage;

    use super::*;

    fn run_start(json: &str) -> RunStart {
        match serde_json::from_str(json).unwrap() {
            InboundMessage::RunStart(run) => run,
            other => panic!("Wrong message type: {other:?}"),
        }
    }

    #[test]
    fn debug_never_prints_an_environment_value_or_stdin() {
        let run = InboundMessage::RunStart(
            RunStart::new("job", "/tmp", "env").with_environment([("APP_MASTER_KEY", "hunter2")]),
        );
        let stdin = InboundMessage::RunStdin(RunStdin::new("job", "aHVudGVyMgo="));

        let run = format!("{run:?}");
        let stdin = format!("{stdin:?}");

        assert!(run.contains("APP_MASTER_KEY"), "{run}");
        assert!(!run.contains("hunter2"), "{run}");
        assert!(stdin.contains("<12 bytes>"), "{stdin}");
        assert!(!stdin.contains("aHVudGVyMgo="), "{stdin}");
    }

    #[test]
    fn test_hello_serialization() {
        let message = InboundMessage::from(Hello::new("1.0").with_capabilities(["cancel"]));

        let json = serde_json::to_string(&message).unwrap();
        assert!(json.contains(r#""type":"Hello""#));
        assert!(json.contains(r#""protocol_version":"1.0""#));
    }

    #[test]
    fn test_hello_deserialization_with_empty_capabilities() {
        let json = r#"{"type": "Hello", "protocol_version": "1.0", "capabilities": []}"#;
        let message: InboundMessage = serde_json::from_str(json).unwrap();

        assert_eq!(message, InboundMessage::Hello(Hello::new("1.0")));
    }

    #[test]
    fn test_hello_deserialization_without_capabilities() {
        let json = r#"{"type": "Hello", "protocol_version": "2.0"}"#;
        let message: InboundMessage = serde_json::from_str(json).unwrap();

        assert_eq!(message, InboundMessage::Hello(Hello::new("2.0")));
    }

    #[test]
    fn test_hello_deserialization_with_all_capabilities() {
        let json = r#"{"type": "Hello", "protocol_version": "1.0", "capabilities": ["cancel", "stdin", "logs", "process_group"]}"#;
        let message: InboundMessage = serde_json::from_str(json).unwrap();

        match message {
            InboundMessage::Hello(hello) => {
                assert_eq!(hello.capabilities.len(), 4);
                assert!(hello.capabilities.contains(&"cancel".to_string()));
                assert!(hello.capabilities.contains(&"stdin".to_string()));
            }
            other => panic!("Wrong message type: {other:?}"),
        }
    }

    #[test]
    fn test_run_start_deserialization() {
        let run = run_start(
            r#"{
                "type": "RunStart",
                "job_id": "job-123",
                "workspace": "/tmp/work",
                "command": "echo",
                "args": ["hello", "world"],
                "env": {"FOO": "bar"}
            }"#,
        );

        assert_eq!(run.job_id, "job-123");
        assert_eq!(run.command, "echo");
        assert_eq!(run.arguments, vec!["hello", "world"]);
        assert_eq!(run.environment.get("FOO"), Some(&"bar".to_string()));
    }

    #[test]
    fn test_run_start_minimal() {
        let run = run_start(
            r#"{
                "type": "RunStart",
                "job_id": "job-456",
                "workspace": "/tmp",
                "command": "ls"
            }"#,
        );

        assert_eq!(run, RunStart::new("job-456", "/tmp", "ls"));
        assert!(run.arguments.is_empty());
        assert!(run.environment.is_empty());
        assert!(run.timeout.is_none());
        assert!(run.output_limit.is_none());
        assert!(run.working_directory.is_none());
    }

    #[test]
    fn test_run_start_with_all_fields() {
        let run = run_start(
            r#"{
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
            }"#,
        );

        assert_eq!(
            run,
            RunStart::new("full-job", "/home/user/project", "bun")
                .with_arguments(["install"])
                .with_environment([("NODE_ENV", "production"), ("CI", "true")])
                .with_timeout(Duration::from_secs(300))
                .with_output_limit(10_485_760)
                .with_working_directory("/home/user/project/packages/app")
                .with_confinement(ConfinementRequest::new(
                    vec![PathBuf::from("/home/user/project")],
                    vec![PathBuf::from("/home/user/project/target")],
                ))
        );
    }

    #[test]
    fn test_run_start_with_unicode_args() {
        let run = run_start(
            r#"{
                "type": "RunStart",
                "job_id": "unicode-job",
                "workspace": "/tmp",
                "command": "echo",
                "args": ["你好", "мир", "🌍"]
            }"#,
        );

        assert_eq!(run.arguments, vec!["你好", "мир", "🌍"]);
    }

    #[test]
    fn test_run_start_with_special_chars_in_env() {
        let run = run_start(
            r#"{
                "type": "RunStart",
                "job_id": "special-env",
                "workspace": "/tmp",
                "command": "bash",
                "env": {"PATH": "/usr/bin:/usr/local/bin", "MSG": "hello=world&foo=bar"}
            }"#,
        );

        assert_eq!(
            run.environment.get("PATH"),
            Some(&"/usr/bin:/usr/local/bin".to_string())
        );
        assert_eq!(
            run.environment.get("MSG"),
            Some(&"hello=world&foo=bar".to_string())
        );
    }

    #[test]
    fn test_run_stdin_deserialization() {
        let json = r#"{"type": "RunStdin", "job_id": "job-123", "data": "SGVsbG8gV29ybGQK", "eof": false}"#;
        let message: InboundMessage = serde_json::from_str(json).unwrap();

        assert_eq!(
            message,
            InboundMessage::RunStdin(RunStdin::new("job-123", "SGVsbG8gV29ybGQK"))
        );
    }

    #[test]
    fn test_run_stdin_with_eof() {
        let json = r#"{"type": "RunStdin", "job_id": "job-123", "data": "", "eof": true}"#;
        let message: InboundMessage = serde_json::from_str(json).unwrap();

        assert_eq!(
            message,
            InboundMessage::RunStdin(RunStdin::new("job-123", "").with_eof(true))
        );
    }

    #[test]
    fn test_run_stdin_default_eof() {
        let json = r#"{"type": "RunStdin", "job_id": "job-123", "data": "dGVzdA=="}"#;
        let message: InboundMessage = serde_json::from_str(json).unwrap();

        assert!(matches!(message, InboundMessage::RunStdin(stdin) if !stdin.eof));
    }

    #[test]
    fn test_run_cancel_deserialization() {
        let json = r#"{"type": "RunCancel", "job_id": "job-123", "force": true}"#;
        let message: InboundMessage = serde_json::from_str(json).unwrap();

        assert_eq!(
            message,
            InboundMessage::RunCancel(RunCancel::new("job-123").with_force(true))
        );
    }

    #[test]
    fn test_run_cancel_default_force() {
        let json = r#"{"type": "RunCancel", "job_id": "job-123"}"#;
        let message: InboundMessage = serde_json::from_str(json).unwrap();

        assert!(matches!(message, InboundMessage::RunCancel(cancel) if !cancel.force));
    }

    #[test]
    fn test_run_cancel_non_force() {
        let json = r#"{"type": "RunCancel", "job_id": "job-789", "force": false}"#;
        let message: InboundMessage = serde_json::from_str(json).unwrap();

        assert_eq!(
            message,
            InboundMessage::RunCancel(RunCancel::new("job-789"))
        );
    }

    #[test]
    fn test_ping_pong() {
        let ping = InboundMessage::Ping(Ping::new("ping-1"));
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
        let original = InboundMessage::Ping(Ping::new("test-ping-123"));
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
        let run = run_start(
            r#"{"type": "RunStart", "job_id": "", "workspace": "/tmp", "command": "ls"}"#,
        );

        assert_eq!(run.job_id, "");
    }

    #[test]
    fn test_very_long_job_id() {
        let long_id = "a".repeat(1000);
        let run = run_start(&format!(
            r#"{{"type": "RunStart", "job_id": "{}", "workspace": "/tmp", "command": "ls"}}"#,
            long_id
        ));

        assert_eq!(run.job_id.len(), 1000);
    }

    #[test]
    fn test_zero_timeout() {
        let run = run_start(
            r#"{
                "type": "RunStart",
                "job_id": "job-1",
                "workspace": "/tmp",
                "command": "ls",
                "timeout_ms": 0
            }"#,
        );

        assert_eq!(run.timeout, Some(Duration::ZERO));
    }

    #[test]
    fn test_max_timeout() {
        let run = run_start(&format!(
            r#"{{"type": "RunStart", "job_id": "job-1", "workspace": "/tmp", "command": "ls", "timeout_ms": {}}}"#,
            u64::MAX
        ));

        assert_eq!(run.timeout, Some(Duration::from_millis(u64::MAX)));
    }

    #[test]
    fn each_message_converts_into_its_variant() {
        assert!(matches!(
            InboundMessage::from(RunStart::new("job", "/tmp", "ls")),
            InboundMessage::RunStart(_)
        ));
        assert!(matches!(
            InboundMessage::from(RunStdin::new("job", "")),
            InboundMessage::RunStdin(_)
        ));
        assert!(matches!(
            InboundMessage::from(RunCancel::new("job")),
            InboundMessage::RunCancel(_)
        ));
        assert!(matches!(
            InboundMessage::from(Ping::new("id")),
            InboundMessage::Ping(_)
        ));
        assert!(matches!(
            InboundMessage::from(Hello::new("1.0")),
            InboundMessage::Hello(_)
        ));
    }
}
