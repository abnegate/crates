//! Integration tests for the command executor.
//!
//! These tests exercise the full execution pipeline including:
//! - Command spawning and output streaming
//! - Timeout handling
//! - Cancellation
//! - Process group management
//! - Output limiting

use std::time::Duration;

use abnegate_exec::executor::CommandExecutor;
use abnegate_exec::executor::ExecutorConfig;
use abnegate_exec::job::JobRegistry;
use abnegate_exec::job::JobState;
use abnegate_exec::protocol::ErrorCode;
use abnegate_exec::protocol::InboundMessage;
use abnegate_exec::protocol::LogLevel;
use abnegate_exec::protocol::OutboundMessage;
use abnegate_exec::protocol::RunStart;
use base64::prelude::*;
use tokio::sync::mpsc;

fn create_echo_request(job_id: &str, message: &str) -> InboundMessage {
    InboundMessage::RunStart(
        RunStart::new(job_id, "/tmp", "echo")
            .with_arguments([message])
            .with_timeout(Duration::from_secs(5)),
    )
}

fn create_bash_request(job_id: &str, script: &str) -> InboundMessage {
    InboundMessage::RunStart(
        RunStart::new(job_id, "/tmp", "bash")
            .with_arguments(["-c", script])
            .with_timeout(Duration::from_secs(30)),
    )
}

fn decode_output_data(data: &str) -> String {
    let bytes = BASE64_STANDARD.decode(data).unwrap();
    String::from_utf8(bytes).unwrap()
}

async fn collect_messages(
    receiver: &mut mpsc::Receiver<OutboundMessage>,
    timeout: Duration,
) -> Vec<OutboundMessage> {
    let mut messages = Vec::new();
    let deadline = tokio::time::Instant::now() + timeout;
    let got_terminal = false;

    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }

        match tokio::time::timeout(remaining, receiver.recv()).await {
            Ok(Some(message)) => {
                let is_terminal = matches!(
                    message,
                    OutboundMessage::RunExit { .. } | OutboundMessage::RunError { .. }
                );
                messages.push(message);
                if is_terminal {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    while let Ok(message) = receiver.try_recv() {
                        messages.push(message);
                    }
                    break;
                }
            }
            Ok(None) => break,
            Err(_) => {
                if got_terminal {
                    break;
                }
            }
        }
    }

    messages
}

#[tokio::test]
async fn test_echo_command() {
    let executor = CommandExecutor::new();
    let (sender, mut receiver) = mpsc::channel(100);

    let request = create_echo_request("echo-1", "Hello World");
    let _handle = executor.spawn(&request, sender).await.unwrap();

    tokio::time::sleep(Duration::from_millis(200)).await;

    let messages = collect_messages(&mut receiver, Duration::from_secs(5)).await;

    assert!(
        messages
            .iter()
            .any(|message| matches!(message, OutboundMessage::RunStarted { job_id, .. } if job_id == "echo-1"))
    );

    let stdout_data: String = messages
        .iter()
        .filter_map(|message| match message {
            OutboundMessage::RunStdout { data, .. } => Some(decode_output_data(data)),
            _ => None,
        })
        .collect();
    assert!(stdout_data.contains("Hello World"));

    assert!(messages.iter().any(|message| matches!(
        message,
        OutboundMessage::RunExit {
            exit_code: Some(0),
            ..
        }
    )));
}

#[tokio::test]
async fn test_stderr_output() {
    let executor = CommandExecutor::new();
    let (sender, mut receiver) = mpsc::channel(100);

    let request = create_bash_request("stderr-1", "echo 'error message' >&2");
    let _handle = executor.spawn(&request, sender).await.unwrap();

    let messages = collect_messages(&mut receiver, Duration::from_secs(5)).await;

    let stderr_data: String = messages
        .iter()
        .filter_map(|message| match message {
            OutboundMessage::RunStderr { data, .. } => Some(decode_output_data(data)),
            _ => None,
        })
        .collect();
    assert!(stderr_data.contains("error message"));
}

#[tokio::test]
async fn test_mixed_stdout_stderr() {
    let executor = CommandExecutor::new();
    let (sender, mut receiver) = mpsc::channel(100);

    let request = create_bash_request("mixed-1", "echo stdout1; echo stderr1 >&2; echo stdout2");
    let _handle = executor.spawn(&request, sender).await.unwrap();

    let messages = collect_messages(&mut receiver, Duration::from_secs(5)).await;

    let stdout_data: String = messages
        .iter()
        .filter_map(|message| match message {
            OutboundMessage::RunStdout { data, .. } => Some(decode_output_data(data)),
            _ => None,
        })
        .collect();

    let stderr_data: String = messages
        .iter()
        .filter_map(|message| match message {
            OutboundMessage::RunStderr { data, .. } => Some(decode_output_data(data)),
            _ => None,
        })
        .collect();

    assert!(stdout_data.contains("stdout1"));
    assert!(stdout_data.contains("stdout2"));
    assert!(stderr_data.contains("stderr1"));
}

#[tokio::test]
async fn test_non_zero_exit_code() {
    let executor = CommandExecutor::new();
    let (sender, mut receiver) = mpsc::channel(100);

    let request = create_bash_request("exit-1", "exit 42");
    let _handle = executor.spawn(&request, sender).await.unwrap();

    let messages = collect_messages(&mut receiver, Duration::from_secs(5)).await;

    assert!(messages.iter().any(|message| matches!(
        message,
        OutboundMessage::RunExit {
            exit_code: Some(42),
            ..
        }
    )));
}

#[tokio::test]
async fn test_command_with_arguments() {
    let executor = CommandExecutor::new();
    let (sender, mut receiver) = mpsc::channel(100);

    let request = InboundMessage::RunStart(
        RunStart::new("args-1", "/tmp", "printf")
            .with_arguments(["%s-%s-%s", "a", "b", "c"])
            .with_timeout(Duration::from_secs(5)),
    );

    let _handle = executor.spawn(&request, sender).await.unwrap();
    let messages = collect_messages(&mut receiver, Duration::from_secs(5)).await;

    let stdout_data: String = messages
        .iter()
        .filter_map(|message| match message {
            OutboundMessage::RunStdout { data, .. } => Some(decode_output_data(data)),
            _ => None,
        })
        .collect();

    assert!(stdout_data.contains("a-b-c"));
}

#[tokio::test]
async fn test_environment_variables() {
    let executor = CommandExecutor::new();
    let (sender, mut receiver) = mpsc::channel(100);

    let request = InboundMessage::RunStart(
        RunStart::new("env-1", "/tmp", "bash")
            .with_arguments(["-c", "echo $MY_VAR"])
            .with_environment([("MY_VAR", "test_value_123")])
            .with_timeout(Duration::from_secs(5)),
    );

    let _handle = executor.spawn(&request, sender).await.unwrap();
    let messages = collect_messages(&mut receiver, Duration::from_secs(5)).await;

    let stdout_data: String = messages
        .iter()
        .filter_map(|message| match message {
            OutboundMessage::RunStdout { data, .. } => Some(decode_output_data(data)),
            _ => None,
        })
        .collect();

    assert!(stdout_data.contains("test_value_123"));
}

#[tokio::test]
async fn test_custom_working_dir() {
    let executor = CommandExecutor::new();
    let (sender, mut receiver) = mpsc::channel(100);

    let temp_dir = tempfile::tempdir().unwrap();
    let temp_path = temp_dir.path().to_path_buf();

    let request = InboundMessage::RunStart(
        RunStart::new("cwd-1", temp_path.clone(), "pwd")
            .with_timeout(Duration::from_secs(5))
            .with_working_directory(temp_path.clone()),
    );

    let _handle = executor.spawn(&request, sender).await.unwrap();
    let messages = collect_messages(&mut receiver, Duration::from_secs(5)).await;

    let stdout_data: String = messages
        .iter()
        .filter_map(|message| match message {
            OutboundMessage::RunStdout { data, .. } => Some(decode_output_data(data)),
            _ => None,
        })
        .collect();

    assert!(stdout_data.contains(temp_path.to_str().unwrap()));
}

#[tokio::test]
async fn test_command_timeout() {
    let executor = CommandExecutor::with_config(ExecutorConfig {
        default_timeout: Duration::from_secs(1),
        grace_period: Duration::from_millis(100),
        ..Default::default()
    });
    let (sender, mut receiver) = mpsc::channel(100);

    let request = InboundMessage::RunStart(
        RunStart::new("timeout-1", "/tmp", "sleep")
            .with_arguments(["10"])
            .with_timeout(Duration::from_millis(500)),
    );

    let _handle = executor.spawn(&request, sender).await.unwrap();
    let messages = collect_messages(&mut receiver, Duration::from_secs(5)).await;

    assert!(messages.iter().any(|message| matches!(
        message,
        OutboundMessage::RunError {
            error_code: ErrorCode::Timeout,
            ..
        }
    )));

    assert!(messages.iter().any(|message| matches!(
        message,
        OutboundMessage::RunLog { level: LogLevel::Warn, message, .. } if message.contains("timed out")
    )));
}

#[tokio::test]
async fn test_invalid_workspace() {
    let executor = CommandExecutor::new();
    let (sender, _receiver) = mpsc::channel(100);

    let request = InboundMessage::RunStart(RunStart::new(
        "bad-ws-1",
        "/nonexistent/path/that/does/not/exist",
        "ls",
    ));

    let result = executor.spawn(&request, sender).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_invalid_command() {
    let executor = CommandExecutor::new();
    let (sender, _receiver) = mpsc::channel(100);

    let request = InboundMessage::RunStart(
        RunStart::new(
            "bad-cmd-1",
            "/tmp",
            "nonexistent_command_that_does_not_exist_12345",
        )
        .with_timeout(Duration::from_secs(5)),
    );

    let result = executor.spawn(&request, sender).await;

    assert!(result.is_err());
}

#[tokio::test]
async fn test_output_limit() {
    let executor = CommandExecutor::new();
    let (sender, mut receiver) = mpsc::channel(100);

    let request = InboundMessage::RunStart(
        RunStart::new("limit-1", "/tmp", "bash")
            .with_arguments([
                "-c",
                "for index in $(seq 1 300); do echo \"This is line $index of output\"; done",
            ])
            .with_timeout(Duration::from_secs(5))
            .with_output_limit(500),
    );

    let _handle = executor.spawn(&request, sender).await.unwrap();

    tokio::time::sleep(Duration::from_millis(500)).await;

    let messages = collect_messages(&mut receiver, Duration::from_secs(5)).await;

    let total_bytes: usize = messages
        .iter()
        .filter_map(|message| match message {
            OutboundMessage::RunStdout { data, .. } => {
                Some(BASE64_STANDARD.decode(data).unwrap_or_default().len())
            }
            _ => None,
        })
        .sum();

    assert!(
        total_bytes <= 600,
        "Expected output to be truncated to ~500 bytes, got {} bytes",
        total_bytes
    );

    assert!(total_bytes > 0, "Expected some output");

    assert!(
        total_bytes < 5000,
        "Output should have been limited, got {} bytes",
        total_bytes
    );
}

#[tokio::test]
async fn test_sequence_numbers_monotonic() {
    let executor = CommandExecutor::new();
    let (sender, mut receiver) = mpsc::channel(100);

    let request = create_bash_request("seq-1", "for index in 1 2 3 4 5; do echo line$index; done");
    let _handle = executor.spawn(&request, sender).await.unwrap();

    let messages = collect_messages(&mut receiver, Duration::from_secs(5)).await;

    let sequences: Vec<u64> = messages
        .iter()
        .filter_map(|message| match message {
            OutboundMessage::RunStdout { sequence, .. } => Some(*sequence),
            _ => None,
        })
        .collect();

    for index in 1..sequences.len() {
        assert!(
            sequences[index] > sequences[index - 1],
            "Sequence numbers should be monotonically increasing"
        );
    }
}

#[tokio::test]
async fn test_registry_concurrent_jobs() {
    let registry = JobRegistry::new();

    let handles: Vec<_> = (0..10)
        .map(|index| {
            let registry_ref = &registry;
            async move {
                let job_id = format!("concurrent-job-{}", index);
                registry_ref.register(job_id.clone()).unwrap();
                registry_ref
                    .update_state(&job_id, JobState::running(index as u32))
                    .unwrap();
                job_id
            }
        })
        .collect();

    for handle in handles {
        handle.await;
    }

    assert_eq!(registry.total_count(), 10);
    assert_eq!(registry.active_count(), 10);

    for index in 0..5 {
        let job_id = format!("concurrent-job-{}", index);
        registry
            .update_state(&job_id, JobState::completed(0, Duration::from_secs(1)))
            .unwrap();
    }

    assert_eq!(registry.active_count(), 5);
}

#[tokio::test]
async fn test_registry_cancel_token_propagation() {
    let registry = JobRegistry::new();

    let token = registry.register("cancel-test".to_string()).unwrap();
    assert!(!token.is_cancelled());

    registry.cancel("cancel-test", false).unwrap();

    assert!(token.is_cancelled());
}

#[tokio::test]
async fn test_registry_cancel_stops_a_job_spawned_with_its_token() {
    let registry = JobRegistry::new();
    let executor = CommandExecutor::with_config(ExecutorConfig {
        grace_period: Duration::from_millis(100),
        ..Default::default()
    });
    let (sender, mut receiver) = mpsc::channel(100);

    let token = registry.register("registry-cancel".to_string()).unwrap();
    executor
        .spawn_with_cancellation(
            &create_bash_request("registry-cancel", "sleep 30"),
            sender,
            token,
        )
        .await
        .unwrap();
    registry.cancel("registry-cancel", false).unwrap();

    let messages = collect_messages(&mut receiver, Duration::from_secs(5)).await;
    assert!(
        messages.iter().any(|message| matches!(
            message,
            OutboundMessage::RunError {
                error_code: ErrorCode::Cancelled,
                ..
            }
        )),
        "{messages:?}"
    );
}

#[tokio::test]
async fn test_unicode_output() {
    let executor = CommandExecutor::new();
    let (sender, mut receiver) = mpsc::channel(100);

    let request = create_echo_request("unicode-1", "Hello 你好 Привет 🌍");
    let _handle = executor.spawn(&request, sender).await.unwrap();

    let messages = collect_messages(&mut receiver, Duration::from_secs(5)).await;

    let stdout_data: String = messages
        .iter()
        .filter_map(|message| match message {
            OutboundMessage::RunStdout { data, .. } => Some(decode_output_data(data)),
            _ => None,
        })
        .collect();

    assert!(stdout_data.contains("你好"));
    assert!(stdout_data.contains("Привет"));
    assert!(stdout_data.contains("🌍"));
}

#[tokio::test]
async fn test_special_shell_characters() {
    let executor = CommandExecutor::new();
    let (sender, mut receiver) = mpsc::channel(100);

    let request = create_echo_request("special-1", "test $VAR && echo || true; `cmd`");
    let _handle = executor.spawn(&request, sender).await.unwrap();

    let messages = collect_messages(&mut receiver, Duration::from_secs(5)).await;

    let stdout_data: String = messages
        .iter()
        .filter_map(|message| match message {
            OutboundMessage::RunStdout { data, .. } => Some(decode_output_data(data)),
            _ => None,
        })
        .collect();

    assert!(stdout_data.contains("$VAR"));
}

#[tokio::test]
async fn test_duration_tracking() {
    let executor = CommandExecutor::new();
    let (sender, mut receiver) = mpsc::channel(100);

    let request = create_bash_request("duration-1", "sleep 0.15");
    let _handle = executor.spawn(&request, sender).await.unwrap();

    let messages = collect_messages(&mut receiver, Duration::from_secs(5)).await;

    let duration = messages.iter().find_map(|message| match message {
        OutboundMessage::RunExit { duration_ms, .. } => Some(*duration_ms),
        _ => None,
    });

    assert!(duration.is_some());
    let duration_ms = duration.unwrap();

    assert!(
        duration_ms >= 120,
        "Duration should be at least 120ms, got {}ms",
        duration_ms
    );
    assert!(
        duration_ms < 5000,
        "Duration should be less than 5000ms, got {}ms",
        duration_ms
    );
}

#[tokio::test]
async fn test_pid_reported() {
    let executor = CommandExecutor::new();
    let (sender, mut receiver) = mpsc::channel(100);

    let request = create_echo_request("pid-1", "test");
    let _handle = executor.spawn(&request, sender).await.unwrap();

    let messages = collect_messages(&mut receiver, Duration::from_secs(5)).await;

    let pid = messages.iter().find_map(|message| match message {
        OutboundMessage::RunStarted { pid, .. } => Some(*pid),
        _ => None,
    });

    assert!(pid.is_some());
    assert!(pid.unwrap() > 0);
}

#[tokio::test]
async fn test_many_quick_commands() {
    let executor = CommandExecutor::new();

    let mut handles = Vec::new();
    for index in 0..20 {
        let executor = executor.clone();
        let handle = tokio::spawn(async move {
            let (sender, mut receiver) = mpsc::channel(100);
            let request =
                create_echo_request(&format!("rapid-{}", index), &format!("message-{}", index));
            let _ = executor.spawn(&request, sender).await.unwrap();
            let messages = collect_messages(&mut receiver, Duration::from_secs(5)).await;

            messages.iter().any(|message| {
                matches!(
                    message,
                    OutboundMessage::RunExit {
                        exit_code: Some(0),
                        ..
                    }
                )
            })
        });
        handles.push(handle);
    }

    let results: Vec<bool> = futures::future::join_all(handles)
        .await
        .into_iter()
        .map(|result| result.unwrap())
        .collect();

    assert!(results.iter().all(|&result| result));
}

#[tokio::test]
async fn test_command_with_no_output() {
    let executor = CommandExecutor::new();
    let (sender, mut receiver) = mpsc::channel(100);

    let request = create_bash_request("no-output-1", "true");
    let _handle = executor.spawn(&request, sender).await.unwrap();

    let messages = collect_messages(&mut receiver, Duration::from_secs(5)).await;

    assert!(
        messages
            .iter()
            .any(|message| matches!(message, OutboundMessage::RunStarted { .. }))
    );
    assert!(messages.iter().any(|message| matches!(
        message,
        OutboundMessage::RunExit {
            exit_code: Some(0),
            ..
        }
    )));

    let has_output = messages.iter().any(|message| {
        matches!(
            message,
            OutboundMessage::RunStdout { .. } | OutboundMessage::RunStderr { .. }
        )
    });
    assert!(!has_output);
}

#[tokio::test]
async fn test_large_output() {
    let executor = CommandExecutor::new();
    let (sender, mut receiver) = mpsc::channel(1000);

    let request = create_bash_request(
        "large-1",
        "for index in $(seq 1 1000); do echo 'This is a test line with some content'; done",
    );
    let _handle = executor.spawn(&request, sender).await.unwrap();

    let messages = collect_messages(&mut receiver, Duration::from_secs(10)).await;

    let total_bytes: usize = messages
        .iter()
        .filter_map(|message| match message {
            OutboundMessage::RunStdout { data, .. } => {
                Some(BASE64_STANDARD.decode(data).unwrap().len())
            }
            _ => None,
        })
        .sum();

    assert!(
        total_bytes > 30_000,
        "Expected at least 30KB of output, got {} bytes",
        total_bytes
    );

    assert!(messages.iter().any(|message| matches!(
        message,
        OutboundMessage::RunExit {
            exit_code: Some(0),
            ..
        }
    )));
}
