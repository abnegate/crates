//! Command execution with streaming output.

use crate::error::ExecutorError;
use crate::protocol::{InboundMessage, LogLevel, OutboundMessage};
use crate::proxy::Proxy;
use base64::prelude::*;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;

use super::confinement::Confinement;
use super::limits::{ExecutorConfig, OutputLimiter};
use super::process_group::ProcessGroup;

/// Handle to a running job's stdin
#[derive(Debug)]
pub struct StdinHandle {
    tx: mpsc::Sender<Vec<u8>>,
}

impl StdinHandle {
    /// Send data to the process's stdin
    pub async fn send(&self, data: Vec<u8>) -> Result<(), ExecutorError> {
        self.tx
            .send(data)
            .await
            .map_err(|_| ExecutorError::ChannelClosed)
    }

    /// Close the stdin (signals EOF to the process)
    pub fn close(self) {
        drop(self);
    }
}

/// Handle to a running job
#[derive(Debug)]
pub struct JobHandle {
    /// Process ID
    pub pid: u32,

    /// Process group for signaling
    pub process_group: ProcessGroup,

    /// Handle to send data to stdin
    pub stdin: Option<StdinHandle>,

    /// Start time
    pub started_at: Instant,

    /// Cancellation flag
    pub cancelled: Arc<AtomicBool>,
}

impl JobHandle {
    /// Get the process ID
    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// Cancel the job
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }

    /// Check if cancelled
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    /// Get elapsed time since start
    pub fn elapsed(&self) -> Duration {
        self.started_at.elapsed()
    }
}

/// Output stream type
#[derive(Debug, Clone, Copy)]
pub enum OutputKind {
    Stdout,
    Stderr,
}

/// Command executor that spawns processes and streams output.
#[derive(Debug, Clone)]
pub struct CommandExecutor {
    config: ExecutorConfig,
}

impl CommandExecutor {
    /// Create a new command executor with default config
    pub fn new() -> Self {
        Self {
            config: ExecutorConfig::default(),
        }
    }

    /// Create a new command executor with custom config
    pub fn with_config(config: ExecutorConfig) -> Self {
        Self { config }
    }

    /// Get the config
    pub fn config(&self) -> &ExecutorConfig {
        &self.config
    }

    /// Spawn a command and start streaming output.
    ///
    /// Returns a job handle that can be used to cancel the job.
    /// Output is sent through the provided channel.
    pub async fn spawn(
        &self,
        request: &InboundMessage,
        tx: mpsc::Sender<OutboundMessage>,
    ) -> Result<JobHandle, ExecutorError> {
        let (
            job_id,
            workspace,
            command,
            args,
            env,
            working_dir,
            timeout_ms,
            max_output_bytes,
            confinement,
        ) = match request {
            InboundMessage::RunStart {
                job_id,
                workspace,
                command,
                args,
                env,
                working_dir,
                timeout_ms,
                max_output_bytes,
                confinement,
            } => (
                job_id.clone(),
                workspace.clone(),
                command.clone(),
                args.clone(),
                env.clone(),
                working_dir.clone(),
                *timeout_ms,
                *max_output_bytes,
                confinement.clone(),
            ),
            _ => {
                return Err(ExecutorError::InvalidWorkspace(
                    "Expected RunStart message".to_string(),
                ));
            }
        };

        if !workspace.exists() {
            return Err(ExecutorError::InvalidWorkspace(format!(
                "Workspace path does not exist: {}",
                workspace.display()
            )));
        }

        if !workspace.is_dir() {
            return Err(ExecutorError::InvalidWorkspace(format!(
                "Workspace path is not a directory: {}",
                workspace.display()
            )));
        }

        let cwd = working_dir.as_ref().unwrap_or(&workspace);

        // A job that asked to be confined never runs unconfined: an unproven
        // sandbox fails the spawn instead of falling back.
        let mut process = match &confinement {
            Some(request) => {
                Confinement::probe(request.mode()).await?;
                let invocation = Confinement::new(&command, args.clone(), cwd)
                    .with_roots(request)
                    .with_environment(env.clone())
                    .host_invocation()?;

                let mut process = Command::new(&invocation.program);
                process
                    .args(&invocation.arguments)
                    .env_clear()
                    .envs(&invocation.environment);
                process
            }
            None => {
                let mut process = Command::new(&command);
                process.args(&args).envs(env.iter());
                Proxy::from_env().apply(&mut process);
                process
            }
        };

        process
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        // Runs between fork and exec, where only async-signal-safe calls are
        // permitted: `setsid` is one, and the closure allocates nothing.
        #[allow(unsafe_code)]
        unsafe {
            process.pre_exec(|| {
                nix::unistd::setsid().map_err(std::io::Error::other)?;
                Ok(())
            });
        }

        let started_at = Instant::now();
        let mut child = process.spawn().map_err(ExecutorError::SpawnFailed)?;

        let pid = child.id().ok_or_else(|| {
            ExecutorError::SpawnFailed(std::io::Error::other("Process has no PID"))
        })?;

        let process_group = ProcessGroup::new(pid);

        let stdin = child.stdin.take();
        let stdin_handle = if let Some(mut stdin_writer) = stdin {
            let (stdin_tx, mut stdin_rx) = mpsc::channel::<Vec<u8>>(100);

            tokio::spawn(async move {
                while let Some(data) = stdin_rx.recv().await {
                    if stdin_writer.write_all(&data).await.is_err() {
                        break;
                    }
                    if stdin_writer.flush().await.is_err() {
                        break;
                    }
                }
            });

            Some(StdinHandle { tx: stdin_tx })
        } else {
            None
        };

        let _ = tx
            .send(OutboundMessage::RunStarted {
                job_id: job_id.clone(),
                pid,
            })
            .await;

        let cancelled = Arc::new(AtomicBool::new(false));

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();

        let max_output = max_output_bytes.unwrap_or(self.config.max_output_bytes);

        let stdout_task = stdout.map(|stdout_reader| {
            self.spawn_output_streamer(
                job_id.clone(),
                stdout_reader,
                OutputKind::Stdout,
                tx.clone(),
                cancelled.clone(),
                max_output,
            )
        });

        let stderr_task = stderr.map(|stderr_reader| {
            self.spawn_output_streamer(
                job_id.clone(),
                stderr_reader,
                OutputKind::Stderr,
                tx.clone(),
                cancelled.clone(),
                max_output,
            )
        });

        let timeout = timeout_ms
            .map(Duration::from_millis)
            .unwrap_or(self.config.default_timeout);
        let grace_period = self.config.grace_period;
        let pg = process_group.clone();
        let job_id_clone = job_id.clone();
        let cancelled_clone = cancelled.clone();

        tokio::spawn(async move {
            let drain_output = async {
                if let Some(task) = stdout_task {
                    let _ = task.await;
                }
                if let Some(task) = stderr_task {
                    let _ = task.await;
                }
            };

            tokio::select! {
                exit_result = child.wait() => {
                    drain_output.await;
                    match exit_result {
                        Ok(status) => {
                            let duration_ms = started_at.elapsed().as_millis() as u64;
                            let _ = tx.send(OutboundMessage::RunExit {
                                job_id: job_id_clone,
                                exit_code: status.code(),
                                signal: None, // TODO: extract signal from status
                                duration_ms,
                            }).await;
                        }
                        Err(e) => {
                            let _ = tx.send(OutboundMessage::error(
                                job_id_clone,
                                crate::protocol::ErrorCode::InternalError,
                                format!("Wait failed: {}", e),
                            )).await;
                        }
                    }
                }

                _ = tokio::time::sleep(timeout) => {
                    if !cancelled_clone.load(Ordering::SeqCst) {
                        let _ = tx.send(OutboundMessage::log(
                            job_id_clone.clone(),
                            LogLevel::Warn,
                            format!("Command timed out after {}ms, killing", timeout.as_millis()),
                            None,
                        )).await;

                        let _ = pg.graceful_kill(grace_period).await;

                        let _ = tx.send(OutboundMessage::error(
                            job_id_clone,
                            crate::protocol::ErrorCode::Timeout,
                            format!("Command timed out after {}ms", timeout.as_millis()),
                        )).await;
                    }
                }

                _ = async {
                    loop {
                        if cancelled_clone.load(Ordering::SeqCst) {
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                } => {
                    let _ = pg.graceful_kill(grace_period).await;

                    let duration_ms = started_at.elapsed().as_millis() as u64;
                    let _ = tx.send(OutboundMessage::error(
                        job_id_clone,
                        crate::protocol::ErrorCode::Cancelled,
                        format!("Command cancelled after {}ms", duration_ms),
                    )).await;
                }
            }
        });

        Ok(JobHandle {
            pid,
            process_group,
            stdin: stdin_handle,
            started_at,
            cancelled,
        })
    }

    /// Spawn a task to stream output from a reader
    fn spawn_output_streamer<R>(
        &self,
        job_id: String,
        reader: R,
        kind: OutputKind,
        tx: mpsc::Sender<OutboundMessage>,
        cancelled: Arc<AtomicBool>,
        max_output_bytes: usize,
    ) -> tokio::task::JoinHandle<()>
    where
        R: tokio::io::AsyncRead + Unpin + Send + 'static,
    {
        let buffer_size = self.config.buffer_size;

        tokio::spawn(async move {
            let mut reader = BufReader::with_capacity(buffer_size, reader);
            let mut limiter = OutputLimiter::new(max_output_bytes);
            let mut sequence = 0u64;

            loop {
                if cancelled.load(Ordering::SeqCst) {
                    break;
                }

                let mut line = String::new();
                match reader.read_line(&mut line).await {
                    Ok(0) => break,
                    Ok(n) => {
                        let (can_write, bytes_to_write, should_warn) = limiter.check(n);

                        if should_warn {
                            let _ = tx
                                .send(OutboundMessage::log(
                                    job_id.clone(),
                                    LogLevel::Warn,
                                    format!(
                                        "Output truncated at {} bytes",
                                        limiter.bytes_written()
                                    ),
                                    None,
                                ))
                                .await;
                        }

                        if can_write && bytes_to_write > 0 {
                            let data = if bytes_to_write < n {
                                &line[..bytes_to_write]
                            } else {
                                &line
                            };

                            let encoded = BASE64_STANDARD.encode(data.as_bytes());
                            sequence += 1;

                            let msg = match kind {
                                OutputKind::Stdout => OutboundMessage::RunStdout {
                                    job_id: job_id.clone(),
                                    data: encoded,
                                    sequence,
                                },
                                OutputKind::Stderr => OutboundMessage::RunStderr {
                                    job_id: job_id.clone(),
                                    data: encoded,
                                    sequence,
                                },
                            };

                            if tx.send(msg).await.is_err() {
                                break;
                            }
                        }

                        if !can_write {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        })
    }
}

impl Default for CommandExecutor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::path::PathBuf;

    #[tokio::test]
    async fn proxy_overrides_request_environment() {
        const NAME: &str = "executor::command::tests::proxy_overrides_request_environment";
        if std::env::var("ABNEGATE_EXEC_TEST_CHILD").as_deref() != Ok(NAME) {
            let output = Command::new(std::env::current_exe().unwrap())
                .args(["--exact", NAME, "--nocapture"])
                .env_clear()
                .env("PATH", std::env::var_os("PATH").unwrap_or_default())
                .env("ABNEGATE_EXEC_TEST_CHILD", NAME)
                .env(crate::proxy::PROXY_URL_ENV, "http://127.0.0.1:28888")
                .output()
                .await
                .unwrap();
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }

        let request = InboundMessage::RunStart {
            job_id: "proxy-environment".to_string(),
            workspace: std::env::temp_dir(),
            command: "env".to_string(),
            args: vec![],
            env: HashMap::from([
                ("HTTPS_PROXY".to_string(), "http://wrong:8888".to_string()),
                ("http_proxy".to_string(), "http://wrong:8888".to_string()),
                ("NO_PROXY".to_string(), "*".to_string()),
                ("no_proxy".to_string(), "*".to_string()),
                (crate::proxy::PROXY_URL_ENV.to_string(), String::new()),
            ]),
            working_dir: None,
            confinement: None,
            timeout_ms: Some(5000),
            max_output_bytes: None,
        };
        let (sender, mut receiver) = mpsc::channel(100);
        CommandExecutor::new()
            .spawn(&request, sender)
            .await
            .unwrap();
        let output = tokio::time::timeout(Duration::from_secs(5), async {
            let mut output = String::new();
            while let Some(message) = receiver.recv().await {
                match message {
                    OutboundMessage::RunStdout { data, .. } => output.push_str(
                        &String::from_utf8(BASE64_STANDARD.decode(data).unwrap()).unwrap(),
                    ),
                    OutboundMessage::RunExit { exit_code, .. } => {
                        assert_eq!(exit_code, Some(0));
                        break;
                    }
                    _ => {}
                }
            }
            output
        })
        .await
        .unwrap();
        for key in [
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "ALL_PROXY",
            "http_proxy",
            "https_proxy",
            "all_proxy",
        ] {
            assert!(
                output
                    .lines()
                    .any(|line| line == format!("{key}=http://127.0.0.1:28888")),
                "{output}"
            );
        }
        assert!(
            !output
                .lines()
                .any(|line| line == "NO_PROXY=*" || line == "no_proxy=*")
        );
        assert!(output.contains("NO_PROXY=localhost,127.0.0.1,::1"));
    }

    #[test]
    fn test_executor_new() {
        let executor = CommandExecutor::new();
        assert_eq!(executor.config().default_timeout, Duration::from_secs(300));
    }

    #[test]
    fn test_executor_with_config() {
        let config = ExecutorConfig::default()
            .with_timeout(Duration::from_secs(60))
            .with_max_output(1024);
        let executor = CommandExecutor::with_config(config);

        assert_eq!(executor.config().default_timeout, Duration::from_secs(60));
        assert_eq!(executor.config().max_output_bytes, 1024);
    }

    #[test]
    fn test_executor_default() {
        let executor: CommandExecutor = Default::default();
        assert_eq!(executor.config().default_timeout, Duration::from_secs(300));
    }

    #[test]
    fn test_executor_config_getter() {
        let config = ExecutorConfig::default().with_buffer_size(4096);
        let executor = CommandExecutor::with_config(config);

        assert_eq!(executor.config().buffer_size, 4096);
    }

    #[tokio::test]
    async fn test_job_handle_cancel() {
        let executor = CommandExecutor::new();
        let (tx, _rx) = mpsc::channel(100);

        let request = InboundMessage::RunStart {
            job_id: "cancel-test".to_string(),
            workspace: PathBuf::from("/tmp"),
            command: "sleep".to_string(),
            args: vec!["10".to_string()],
            env: HashMap::new(),
            timeout_ms: Some(30000),
            max_output_bytes: None,
            working_dir: None,
            confinement: None,
        };

        let handle = executor.spawn(&request, tx).await.unwrap();
        assert!(!handle.is_cancelled());

        handle.cancel();
        assert!(handle.is_cancelled());
    }

    #[tokio::test]
    async fn test_job_handle_elapsed() {
        let executor = CommandExecutor::new();
        let (tx, _rx) = mpsc::channel(100);

        let request = InboundMessage::RunStart {
            job_id: "elapsed-test".to_string(),
            workspace: PathBuf::from("/tmp"),
            command: "echo".to_string(),
            args: vec!["test".to_string()],
            env: HashMap::new(),
            timeout_ms: Some(5000),
            max_output_bytes: None,
            working_dir: None,
            confinement: None,
        };

        let handle = executor.spawn(&request, tx).await.unwrap();

        tokio::time::sleep(Duration::from_millis(50)).await;

        let elapsed = handle.elapsed();
        assert!(elapsed >= Duration::from_millis(50));
    }

    #[tokio::test]
    async fn test_duration_spans_the_whole_child() {
        const SLEPT: Duration = Duration::from_millis(100);

        let executor = CommandExecutor::new();
        let (tx, mut rx) = mpsc::channel(100);

        let request = InboundMessage::RunStart {
            job_id: "duration-test".to_string(),
            workspace: PathBuf::from("/tmp"),
            command: "sh".to_string(),
            args: vec!["-c".to_string(), format!("sleep {}", SLEPT.as_secs_f64())],
            env: HashMap::new(),
            timeout_ms: Some(5000),
            max_output_bytes: None,
            working_dir: None,
            confinement: None,
        };

        executor.spawn(&request, tx).await.unwrap();

        let reported = loop {
            let message = rx.recv().await.expect("the run reports how it ended");
            if let OutboundMessage::RunExit { duration_ms, .. } = message {
                break u128::from(duration_ms);
            }
        };

        assert!(
            reported >= SLEPT.as_millis(),
            "a run that slept {}ms is reported as {reported}ms",
            SLEPT.as_millis()
        );
    }

    /// The gap between the spawn and the timestamp holds an `await` on the
    /// outbound channel, so a consumer that is not reading stretches it without
    /// bound. Filling the channel before the spawn holds that `await` open for
    /// longer than the child lives, which turns an understatement of a few
    /// milliseconds into the whole run and fails whenever the timestamp is
    /// taken late.
    #[tokio::test]
    async fn test_duration_survives_a_stalled_consumer() {
        const SLEPT: Duration = Duration::from_millis(100);
        const STALL: Duration = Duration::from_millis(300);

        let executor = CommandExecutor::new();
        let (tx, mut rx) = mpsc::channel(1);
        tx.send(OutboundMessage::log(
            "stall".to_string(),
            LogLevel::Info,
            "stall".to_string(),
            None,
        ))
        .await
        .unwrap();

        let request = InboundMessage::RunStart {
            job_id: "stalled-consumer-test".to_string(),
            workspace: PathBuf::from("/tmp"),
            command: "sh".to_string(),
            args: vec!["-c".to_string(), format!("sleep {}", SLEPT.as_secs_f64())],
            env: HashMap::new(),
            timeout_ms: Some(5000),
            max_output_bytes: None,
            working_dir: None,
            confinement: None,
        };

        let drain = tokio::spawn(async move {
            tokio::time::sleep(STALL).await;
            while let Some(message) = rx.recv().await {
                if let OutboundMessage::RunExit { duration_ms, .. } = message {
                    return Some(u128::from(duration_ms));
                }
            }
            None
        });

        executor.spawn(&request, tx).await.unwrap();
        let reported = drain.await.unwrap().expect("the run reports how it ended");

        assert!(
            reported >= SLEPT.as_millis(),
            "a run that slept {}ms is reported as {reported}ms",
            SLEPT.as_millis()
        );
    }

    #[tokio::test]
    async fn test_stdin_handle_send() {
        let (tx, mut rx) = mpsc::channel::<Vec<u8>>(10);
        let stdin_handle = StdinHandle { tx };

        let result = stdin_handle.send(b"hello".to_vec()).await;
        assert!(result.is_ok());

        let received = rx.recv().await.unwrap();
        assert_eq!(received, b"hello".to_vec());
    }

    #[tokio::test]
    async fn test_stdin_handle_send_closed_channel() {
        let (tx, rx) = mpsc::channel::<Vec<u8>>(10);
        let stdin_handle = StdinHandle { tx };

        drop(rx);

        let result = stdin_handle.send(b"hello".to_vec()).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            ExecutorError::ChannelClosed => {}
            e => panic!("Expected ChannelClosed, got {:?}", e),
        }
    }

    #[tokio::test]
    async fn test_stdin_handle_close() {
        let (tx, mut rx) = mpsc::channel::<Vec<u8>>(10);
        let stdin_handle = StdinHandle { tx };

        stdin_handle.close();

        assert!(rx.recv().await.is_none());
    }

    #[test]
    fn test_output_kind_clone_copy() {
        let kind = OutputKind::Stdout;
        let cloned = kind;
        let copied = kind;

        assert!(matches!(cloned, OutputKind::Stdout));
        assert!(matches!(copied, OutputKind::Stdout));

        let kind = OutputKind::Stderr;
        assert!(matches!(kind, OutputKind::Stderr));
    }

    #[test]
    fn test_output_kind_debug() {
        let kind = OutputKind::Stdout;
        let debug_str = format!("{:?}", kind);
        assert!(debug_str.contains("Stdout"));
    }

    #[tokio::test]
    async fn test_spawn_echo() {
        let executor = CommandExecutor::new();
        let (tx, mut rx) = mpsc::channel(100);

        let request = InboundMessage::RunStart {
            job_id: "test-1".to_string(),
            workspace: PathBuf::from("/tmp"),
            command: "echo".to_string(),
            args: vec!["hello".to_string()],
            env: HashMap::new(),
            timeout_ms: Some(5000),
            max_output_bytes: None,
            working_dir: None,
            confinement: None,
        };

        let handle = executor.spawn(&request, tx).await;
        assert!(handle.is_ok());

        tokio::time::sleep(Duration::from_millis(500)).await;

        let mut got_started = false;
        let mut got_stdout = false;
        let mut got_exit = false;

        while let Ok(msg) = rx.try_recv() {
            match msg {
                OutboundMessage::RunStarted { job_id, .. } => {
                    assert_eq!(job_id, "test-1");
                    got_started = true;
                }
                OutboundMessage::RunStdout { job_id, data, .. } => {
                    assert_eq!(job_id, "test-1");
                    let decoded = BASE64_STANDARD.decode(&data).unwrap();
                    let text = String::from_utf8(decoded).unwrap();
                    assert!(text.contains("hello"));
                    got_stdout = true;
                }
                OutboundMessage::RunExit {
                    job_id, exit_code, ..
                } => {
                    assert_eq!(job_id, "test-1");
                    assert_eq!(exit_code, Some(0));
                    got_exit = true;
                }
                _ => {}
            }
        }

        assert!(got_started);
        assert!(got_stdout);
        assert!(got_exit);
    }

    #[tokio::test]
    async fn test_spawn_with_working_dir() {
        let executor = CommandExecutor::new();
        let (tx, mut rx) = mpsc::channel(100);

        let request = InboundMessage::RunStart {
            job_id: "workdir-test".to_string(),
            workspace: PathBuf::from("/tmp"),
            command: "pwd".to_string(),
            args: vec![],
            env: HashMap::new(),
            timeout_ms: Some(5000),
            max_output_bytes: None,
            working_dir: Some(PathBuf::from("/tmp")),
            confinement: None,
        };

        let handle = executor.spawn(&request, tx).await;
        assert!(handle.is_ok());

        tokio::time::sleep(Duration::from_millis(500)).await;

        let mut got_pwd = false;
        while let Ok(msg) = rx.try_recv() {
            if let OutboundMessage::RunStdout { data, .. } = msg {
                let decoded = BASE64_STANDARD.decode(&data).unwrap();
                let text = String::from_utf8(decoded).unwrap();
                if text.contains("/tmp") || text.contains("/private/tmp") {
                    got_pwd = true;
                }
            }
        }

        assert!(got_pwd);
    }

    #[tokio::test]
    async fn test_spawn_with_env() {
        let executor = CommandExecutor::new();
        let (tx, mut rx) = mpsc::channel(100);

        let mut env = HashMap::new();
        env.insert("MY_VAR".to_string(), "my_value".to_string());

        let request = InboundMessage::RunStart {
            job_id: "env-test".to_string(),
            workspace: PathBuf::from("/tmp"),
            command: "sh".to_string(),
            args: vec!["-c".to_string(), "echo $MY_VAR".to_string()],
            env,
            timeout_ms: Some(5000),
            max_output_bytes: None,
            working_dir: None,
            confinement: None,
        };

        let handle = executor.spawn(&request, tx).await;
        assert!(handle.is_ok());

        tokio::time::sleep(Duration::from_millis(500)).await;

        let mut got_env_value = false;
        while let Ok(msg) = rx.try_recv() {
            if let OutboundMessage::RunStdout { data, .. } = msg {
                let decoded = BASE64_STANDARD.decode(&data).unwrap();
                let text = String::from_utf8(decoded).unwrap();
                if text.contains("my_value") {
                    got_env_value = true;
                }
            }
        }

        assert!(got_env_value);
    }

    #[tokio::test]
    async fn test_spawn_stderr_output() {
        let executor = CommandExecutor::new();
        let (tx, mut rx) = mpsc::channel(100);

        let request = InboundMessage::RunStart {
            job_id: "stderr-test".to_string(),
            workspace: PathBuf::from("/tmp"),
            command: "sh".to_string(),
            args: vec!["-c".to_string(), "echo error >&2".to_string()],
            env: HashMap::new(),
            timeout_ms: Some(5000),
            max_output_bytes: None,
            working_dir: None,
            confinement: None,
        };

        let handle = executor.spawn(&request, tx).await;
        assert!(handle.is_ok());

        let got_stderr_before_exit = tokio::time::timeout(Duration::from_secs(5), async {
            let mut got_stderr = false;
            while let Some(msg) = rx.recv().await {
                if let OutboundMessage::RunStderr { data, .. } = &msg {
                    let decoded = BASE64_STANDARD.decode(data).unwrap();
                    let text = String::from_utf8(decoded).unwrap();
                    if text.contains("error") {
                        got_stderr = true;
                    }
                }
                if matches!(msg, OutboundMessage::RunExit { .. }) {
                    return got_stderr;
                }
            }
            got_stderr
        })
        .await
        .expect("timed out waiting for RunExit");

        assert!(
            got_stderr_before_exit,
            "stderr must be delivered before RunExit"
        );
    }

    #[tokio::test]
    async fn test_invalid_workspace() {
        let executor = CommandExecutor::new();
        let (tx, _rx) = mpsc::channel(100);

        let request = InboundMessage::RunStart {
            job_id: "test-2".to_string(),
            workspace: PathBuf::from("/nonexistent/path"),
            command: "echo".to_string(),
            args: vec![],
            env: HashMap::new(),
            timeout_ms: None,
            max_output_bytes: None,
            working_dir: None,
            confinement: None,
        };

        let result = executor.spawn(&request, tx).await;
        assert!(result.is_err());

        match result.unwrap_err() {
            ExecutorError::InvalidWorkspace(msg) => {
                assert!(msg.contains("does not exist"));
            }
            e => panic!("Wrong error type: {:?}", e),
        }
    }

    #[tokio::test]
    async fn test_spawn_workspace_is_file() {
        let executor = CommandExecutor::new();
        let (tx, _rx) = mpsc::channel(100);

        let request = InboundMessage::RunStart {
            job_id: "file-workspace-test".to_string(),
            workspace: PathBuf::from("/etc/passwd"),
            command: "echo".to_string(),
            args: vec![],
            env: HashMap::new(),
            timeout_ms: None,
            max_output_bytes: None,
            working_dir: None,
            confinement: None,
        };

        let result = executor.spawn(&request, tx).await;
        assert!(result.is_err());

        match result.unwrap_err() {
            ExecutorError::InvalidWorkspace(msg) => {
                assert!(msg.contains("not a directory"));
            }
            e => panic!("Wrong error type: {:?}", e),
        }
    }

    #[tokio::test]
    async fn test_spawn_wrong_message_type() {
        let executor = CommandExecutor::new();
        let (tx, _rx) = mpsc::channel(100);

        let request = InboundMessage::Ping {
            id: "1".to_string(),
        };

        let result = executor.spawn(&request, tx).await;
        assert!(result.is_err());

        match result.unwrap_err() {
            ExecutorError::InvalidWorkspace(msg) => {
                assert!(msg.contains("Expected RunStart"));
            }
            e => panic!("Wrong error type: {:?}", e),
        }
    }

    #[tokio::test]
    async fn test_spawn_non_zero_exit() {
        let executor = CommandExecutor::new();
        let (tx, mut rx) = mpsc::channel(100);

        let request = InboundMessage::RunStart {
            job_id: "nonzero-test".to_string(),
            workspace: PathBuf::from("/tmp"),
            command: "sh".to_string(),
            args: vec!["-c".to_string(), "exit 42".to_string()],
            env: HashMap::new(),
            timeout_ms: Some(5000),
            max_output_bytes: None,
            working_dir: None,
            confinement: None,
        };

        let _handle = executor.spawn(&request, tx).await.unwrap();

        tokio::time::sleep(Duration::from_millis(500)).await;

        let mut exit_code = None;
        while let Ok(msg) = rx.try_recv() {
            if let OutboundMessage::RunExit {
                exit_code: code, ..
            } = msg
            {
                exit_code = code;
            }
        }

        assert_eq!(exit_code, Some(42));
    }

    #[tokio::test]
    async fn test_spawn_invalid_command() {
        let executor = CommandExecutor::new();
        let (tx, _rx) = mpsc::channel(100);

        let request = InboundMessage::RunStart {
            job_id: "invalid-cmd-test".to_string(),
            workspace: PathBuf::from("/tmp"),
            command: "/nonexistent/binary/that/doesnt/exist".to_string(),
            args: vec![],
            env: HashMap::new(),
            timeout_ms: Some(5000),
            max_output_bytes: None,
            working_dir: None,
            confinement: None,
        };

        let result = executor.spawn(&request, tx).await;
        assert!(result.is_err());

        match result.unwrap_err() {
            ExecutorError::SpawnFailed(_) => {}
            e => panic!("Wrong error type: {:?}", e),
        }
    }

    #[tokio::test]
    async fn test_spawn_with_custom_output_limit() {
        let executor = CommandExecutor::new();
        let (tx, mut rx) = mpsc::channel(100);

        let request = InboundMessage::RunStart {
            job_id: "limit-test".to_string(),
            workspace: PathBuf::from("/tmp"),
            command: "sh".to_string(),
            args: vec![
                "-c".to_string(),
                "for i in $(seq 1 100); do echo line$i; done".to_string(),
            ],
            env: HashMap::new(),
            timeout_ms: Some(5000),
            max_output_bytes: Some(100),
            working_dir: None,
            confinement: None,
        };

        let _handle = executor.spawn(&request, tx).await.unwrap();

        tokio::time::sleep(Duration::from_millis(500)).await;

        let mut total_output = 0;
        while let Ok(msg) = rx.try_recv() {
            if let OutboundMessage::RunStdout { data, .. } = msg {
                let decoded = BASE64_STANDARD.decode(&data).unwrap();
                total_output += decoded.len();
            }
        }

        assert!(
            total_output <= 200,
            "Output was not limited: {} bytes",
            total_output
        );
    }
}
