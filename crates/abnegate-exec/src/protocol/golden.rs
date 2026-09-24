//! The exact line each protocol message is written as.
//!
//! A runner and its client may be built from different versions of this
//! crate, so renaming a Rust type or field must never move a byte on the
//! wire. Every map here holds one entry, which fixes its order.

use std::collections::HashMap;
use std::fmt::Debug;
use std::path::PathBuf;

use bytes::BytesMut;
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio_util::codec::Decoder;
use tokio_util::codec::Encoder;

use super::ConfinementRequest;
use super::ErrorCode;
use super::InboundMessage;
use super::LogLevel;
use super::NdjsonCodec;
use super::OutboundMessage;
use super::ProcessTreeRequest;

fn encoded<T: Serialize>(message: T) -> String {
    let mut buffer = BytesMut::new();
    NdjsonCodec::<T>::new()
        .encode(message, &mut buffer)
        .expect("every message serializes");
    String::from_utf8(buffer.to_vec()).expect("NDJSON is UTF-8")
}

fn decoded<T: DeserializeOwned>(line: &str) -> T {
    NdjsonCodec::<T>::new()
        .decode(&mut BytesMut::from(format!("{line}\n").as_bytes()))
        .expect("the line parses")
        .expect("the line is complete")
}

fn assert_golden<T>(message: T, line: &str)
where
    T: Serialize + DeserializeOwned + PartialEq + Debug + Clone,
{
    assert_eq!(encoded(message.clone()), format!("{line}\n"));
    assert_eq!(decoded::<T>(line), message, "{line}");
}

fn full_run() -> InboundMessage {
    InboundMessage::RunStart {
        job_id: "job-1".to_string(),
        workspace: PathBuf::from("/tmp/work"),
        command: "cargo".to_string(),
        args: vec!["test".to_string(), "--quiet".to_string()],
        env: HashMap::from([("RUST_LOG".to_string(), "debug".to_string())]),
        timeout_ms: Some(300_000),
        max_output_bytes: Some(1_048_576),
        working_dir: Some(PathBuf::from("/tmp/work/crate")),
        confinement: Some(Box::new(ConfinementRequest {
            read_roots: vec![PathBuf::from("/tmp/work")],
            write_roots: vec![PathBuf::from("/tmp/work/target")],
            process_tree: Some(ProcessTreeRequest {
                execute_roots: vec![PathBuf::from("/usr/bin")],
            }),
        })),
    }
}

fn minimal_run() -> InboundMessage {
    InboundMessage::RunStart {
        job_id: "job-2".to_string(),
        workspace: PathBuf::from("/tmp"),
        command: "ls".to_string(),
        args: vec![],
        env: HashMap::new(),
        timeout_ms: None,
        max_output_bytes: None,
        working_dir: None,
        confinement: None,
    }
}

fn single_command_run() -> InboundMessage {
    InboundMessage::RunStart {
        job_id: "job-3".to_string(),
        workspace: PathBuf::from("/tmp"),
        command: "/bin/cat".to_string(),
        args: vec!["granted".to_string()],
        env: HashMap::new(),
        timeout_ms: Some(15_000),
        max_output_bytes: None,
        working_dir: None,
        confinement: Some(Box::new(ConfinementRequest {
            read_roots: vec![PathBuf::from("/tmp")],
            write_roots: vec![],
            process_tree: None,
        })),
    }
}

#[test]
fn every_inbound_message_keeps_its_bytes() {
    let cases = [
        (
            InboundMessage::Hello {
                protocol_version: "1.0".to_string(),
                capabilities: vec!["cancel".to_string(), "stdin".to_string()],
            },
            r#"{"type":"Hello","protocol_version":"1.0","capabilities":["cancel","stdin"]}"#,
        ),
        (
            full_run(),
            r#"{"type":"RunStart","job_id":"job-1","workspace":"/tmp/work","command":"cargo","args":["test","--quiet"],"env":{"RUST_LOG":"debug"},"timeout_ms":300000,"max_output_bytes":1048576,"working_dir":"/tmp/work/crate","confinement":{"read_roots":["/tmp/work"],"write_roots":["/tmp/work/target"],"process_tree":{"execute_roots":["/usr/bin"]}}}"#,
        ),
        (
            minimal_run(),
            r#"{"type":"RunStart","job_id":"job-2","workspace":"/tmp","command":"ls","args":[],"env":{},"timeout_ms":null,"max_output_bytes":null,"working_dir":null,"confinement":null}"#,
        ),
        (
            single_command_run(),
            r#"{"type":"RunStart","job_id":"job-3","workspace":"/tmp","command":"/bin/cat","args":["granted"],"env":{},"timeout_ms":15000,"max_output_bytes":null,"working_dir":null,"confinement":{"read_roots":["/tmp"],"write_roots":[]}}"#,
        ),
        (
            InboundMessage::RunStdin {
                job_id: "job-1".to_string(),
                data: "aGVsbG8K".to_string(),
                eof: true,
            },
            r#"{"type":"RunStdin","job_id":"job-1","data":"aGVsbG8K","eof":true}"#,
        ),
        (
            InboundMessage::RunCancel {
                job_id: "job-1".to_string(),
                force: true,
            },
            r#"{"type":"RunCancel","job_id":"job-1","force":true}"#,
        ),
        (
            InboundMessage::Ping {
                id: "ping-1".to_string(),
            },
            r#"{"type":"Ping","id":"ping-1"}"#,
        ),
    ];

    for (message, line) in cases {
        assert_golden(message, line);
    }
}

#[test]
fn every_outbound_message_keeps_its_bytes() {
    let cases = [
        (
            OutboundMessage::HelloAck {
                protocol_version: "1.0".to_string(),
                runner_version: "0.1.0".to_string(),
                capabilities: vec!["cancel".to_string(), "process_group".to_string()],
            },
            r#"{"type":"HelloAck","protocol_version":"1.0","runner_version":"0.1.0","capabilities":["cancel","process_group"]}"#,
        ),
        (
            OutboundMessage::RunStarted {
                job_id: "job-1".to_string(),
                pid: 4242,
            },
            r#"{"type":"RunStarted","job_id":"job-1","pid":4242}"#,
        ),
        (
            OutboundMessage::RunStdout {
                job_id: "job-1".to_string(),
                data: "aGVsbG8K".to_string(),
                sequence: 1,
            },
            r#"{"type":"RunStdout","job_id":"job-1","data":"aGVsbG8K","sequence":1}"#,
        ),
        (
            OutboundMessage::RunStderr {
                job_id: "job-1".to_string(),
                data: "b29wcwo=".to_string(),
                sequence: 2,
            },
            r#"{"type":"RunStderr","job_id":"job-1","data":"b29wcwo=","sequence":2}"#,
        ),
        (
            OutboundMessage::RunLog {
                job_id: "job-1".to_string(),
                level: LogLevel::Warn,
                message: "Output truncated at 100 bytes".to_string(),
                details: Some(serde_json::json!({ "limit": 100 })),
            },
            r#"{"type":"RunLog","job_id":"job-1","level":"warn","message":"Output truncated at 100 bytes","details":{"limit":100}}"#,
        ),
        (
            OutboundMessage::RunLog {
                job_id: "job-1".to_string(),
                level: LogLevel::Info,
                message: "started".to_string(),
                details: None,
            },
            r#"{"type":"RunLog","job_id":"job-1","level":"info","message":"started"}"#,
        ),
        (
            OutboundMessage::RunExit {
                job_id: "job-1".to_string(),
                exit_code: Some(0),
                signal: None,
                duration_ms: 1500,
            },
            r#"{"type":"RunExit","job_id":"job-1","exit_code":0,"duration_ms":1500}"#,
        ),
        (
            OutboundMessage::RunExit {
                job_id: "job-1".to_string(),
                exit_code: None,
                signal: Some(9),
                duration_ms: 20,
            },
            r#"{"type":"RunExit","job_id":"job-1","exit_code":null,"signal":9,"duration_ms":20}"#,
        ),
        (
            OutboundMessage::RunError {
                job_id: "job-1".to_string(),
                error_code: ErrorCode::Timeout,
                message: "Command timed out after 500ms".to_string(),
            },
            r#"{"type":"RunError","job_id":"job-1","error_code":"timeout","message":"Command timed out after 500ms"}"#,
        ),
        (
            OutboundMessage::Pong {
                id: "ping-1".to_string(),
            },
            r#"{"type":"Pong","id":"ping-1"}"#,
        ),
    ];

    for (message, line) in cases {
        assert_golden(message, line);
    }
}

#[test]
fn a_sparse_inbound_line_takes_its_defaults() {
    let cases = [
        (
            r#"{"type":"Hello","protocol_version":"1.0"}"#,
            InboundMessage::Hello {
                protocol_version: "1.0".to_string(),
                capabilities: vec![],
            },
        ),
        (
            r#"{"type":"RunStart","job_id":"job-2","workspace":"/tmp","command":"ls"}"#,
            minimal_run(),
        ),
        (
            r#"{"type":"RunStart","job_id":"job-3","workspace":"/tmp","command":"/bin/cat","args":["granted"],"timeout_ms":15000,"confinement":{"read_roots":["/tmp"]}}"#,
            single_command_run(),
        ),
        (
            r#"{"type":"RunStdin","job_id":"job-1","data":""}"#,
            InboundMessage::RunStdin {
                job_id: "job-1".to_string(),
                data: String::new(),
                eof: false,
            },
        ),
        (
            r#"{"type":"RunCancel","job_id":"job-1"}"#,
            InboundMessage::RunCancel {
                job_id: "job-1".to_string(),
                force: false,
            },
        ),
    ];

    for (line, message) in cases {
        assert_eq!(decoded::<InboundMessage>(line), message, "{line}");
    }
}

#[test]
fn the_handshake_answer_keeps_its_tag() {
    let line = encoded(OutboundMessage::hello_ack());

    assert!(
        line.starts_with(r#"{"type":"HelloAck","protocol_version":"1.0","runner_version":""#),
        "{line}"
    );
}
