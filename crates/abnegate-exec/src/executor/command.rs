//! Command execution with streaming output.

use std::process::Stdio;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::sync::mpsc;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::error::ExecutorError;
use crate::protocol::InboundMessage;
use crate::protocol::OutboundMessage;
use crate::proxy::Proxy;

use super::config::ExecutorConfig;
use super::confinement::Confinement;
use super::job_handle::JobHandle;
use super::output_kind::OutputKind;
use super::output_limiter::OutputLimiter;
use super::output_stream::OutputStream;
use super::process_group::ProcessGroup;
use super::session;
use super::stdin_handle::StdinHandle;
use super::supervisor::Supervisor;

/// Capacity of the queue between a [`StdinHandle`] and the child's stdin.
const STDIN_CAPACITY: usize = 100;

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
        sender: mpsc::Sender<OutboundMessage>,
    ) -> Result<JobHandle, ExecutorError> {
        self.spawn_with_cancellation(request, sender, CancellationToken::new())
            .await
    }

    /// Spawn a command that stops when `cancellation` is cancelled.
    ///
    /// Pass the token [`JobRegistry::register`](crate::job::JobRegistry::register)
    /// returned, so that cancelling the job through the registry stops it.
    pub async fn spawn_with_cancellation(
        &self,
        request: &InboundMessage,
        sender: mpsc::Sender<OutboundMessage>,
        cancellation: CancellationToken,
    ) -> Result<JobHandle, ExecutorError> {
        let InboundMessage::RunStart {
            job_id,
            workspace,
            command,
            args,
            env,
            timeout_ms,
            max_output_bytes,
            working_dir,
            confinement,
        } = request
        else {
            return Err(ExecutorError::NotRunStart);
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

        let working_directory = working_dir.as_ref().unwrap_or(workspace);
        let configure = |process: &mut Command| {
            process
                .current_dir(working_directory)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true);
        };

        // A job that asked to be confined never runs unconfined: an unproven
        // sandbox fails the spawn instead of falling back.
        let inherited = self.config.environment.inherited();
        let spawned = match confinement {
            Some(request) => {
                Confinement::probe(request.mode()).await?;
                Confinement::new(command, args.clone(), working_directory)
                    .with_roots(request)
                    .with_environment(env.clone())
                    .with_inherited_environment(inherited)
                    .host_invocation()?
                    .spawn(configure)
            }
            None => {
                let mut process = Command::new(command);
                process.args(args).env_clear().envs(inherited).envs(env);
                Proxy::from_env().apply(&mut process);
                configure(&mut process);
                session::lead(&mut process, None);
                process.spawn()
            }
        };
        let mut child = spawned.map_err(ExecutorError::SpawnFailed)?;
        let started_at = Instant::now();

        let pid = child.id().ok_or_else(|| {
            ExecutorError::SpawnFailed(std::io::Error::other("Process has no PID"))
        })?;

        let process_group = ProcessGroup::try_from(pid)?;

        let stdin = child.stdin.take().map(|mut writer| {
            let (stdin_sender, mut stdin_receiver) = mpsc::channel::<Vec<u8>>(STDIN_CAPACITY);
            tokio::spawn(async move {
                while let Some(data) = stdin_receiver.recv().await {
                    if writer.write_all(&data).await.is_err() || writer.flush().await.is_err() {
                        break;
                    }
                }
            });
            StdinHandle {
                sender: stdin_sender,
            }
        });

        let (started, gate) = watch::channel(false);
        let limiter = Arc::new(Mutex::new(OutputLimiter::new(
            max_output_bytes.unwrap_or(self.config.max_output_bytes),
        )));
        let stream = |kind: OutputKind| OutputStream {
            job_id: job_id.clone(),
            kind,
            sender: sender.clone(),
            limiter: limiter.clone(),
            buffer_size: self.config.buffer_size,
            started: gate.clone(),
        };
        let mut streams: Vec<JoinHandle<()>> = Vec::with_capacity(2);
        if let Some(stdout) = child.stdout.take() {
            streams.push(stream(OutputKind::Stdout).spawn(stdout, cancellation.clone()));
        }
        if let Some(stderr) = child.stderr.take() {
            streams.push(stream(OutputKind::Stderr).spawn(stderr, cancellation.clone()));
        }

        Supervisor {
            job_id: job_id.clone(),
            sender: sender.clone(),
            process_group: process_group.clone(),
            started_at,
            timeout: timeout_ms
                .map(Duration::from_millis)
                .unwrap_or(self.config.default_timeout),
            grace_period: self.config.grace_period,
            started: gate,
        }
        .spawn(child, streams, cancellation.clone());

        let _ = sender
            .send(OutboundMessage::RunStarted {
                job_id: job_id.clone(),
                pid,
            })
            .await;
        let _ = started.send(true);

        Ok(JobHandle {
            pid,
            process_group,
            stdin,
            started_at,
            cancellation,
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
    use std::collections::HashMap;
    use std::path::Path;
    use std::path::PathBuf;

    use base64::Engine;
    use base64::prelude::BASE64_STANDARD;

    use crate::executor::ConfinementMode;
    use crate::executor::EnvironmentPolicy;
    use crate::executor::GRACE_PERIOD;
    use crate::executor::sandbox;
    use crate::executor::sandbox::REQUIRE_CONFINEMENT;
    use crate::protocol::ConfinementRequest;
    use crate::protocol::ErrorCode;
    use crate::protocol::LogLevel;

    use super::*;

    const CHILD: &str = "ABNEGATE_EXEC_TEST_CHILD";
    const RUN_LIMIT: Duration = Duration::from_secs(10);

    /// Everything one run reported, in the order it arrived.
    #[derive(Default)]
    struct Run {
        messages: Vec<OutboundMessage>,
    }

    impl Run {
        fn stdout(&self) -> Vec<u8> {
            self.bytes(|message| match message {
                OutboundMessage::RunStdout { data, .. } => Some(data),
                _ => None,
            })
        }

        fn stderr(&self) -> Vec<u8> {
            self.bytes(|message| match message {
                OutboundMessage::RunStderr { data, .. } => Some(data),
                _ => None,
            })
        }

        fn bytes(&self, data: impl Fn(&OutboundMessage) -> Option<&String>) -> Vec<u8> {
            self.messages
                .iter()
                .filter_map(data)
                .flat_map(|data| BASE64_STANDARD.decode(data).unwrap())
                .collect()
        }

        fn exit(&self) -> Option<(Option<i32>, Option<i32>)> {
            self.messages.iter().find_map(|message| match message {
                OutboundMessage::RunExit {
                    exit_code, signal, ..
                } => Some((*exit_code, *signal)),
                _ => None,
            })
        }

        fn error(&self) -> Option<ErrorCode> {
            self.messages.iter().find_map(|message| match message {
                OutboundMessage::RunError { error_code, .. } => Some(*error_code),
                _ => None,
            })
        }
    }

    /// Collect messages until the run's terminal message.
    async fn finish(mut receiver: mpsc::Receiver<OutboundMessage>) -> Run {
        tokio::time::timeout(RUN_LIMIT, async {
            let mut run = Run::default();
            while let Some(message) = receiver.recv().await {
                let terminal = matches!(
                    message,
                    OutboundMessage::RunExit { .. } | OutboundMessage::RunError { .. }
                );
                run.messages.push(message);
                if terminal {
                    break;
                }
            }
            run
        })
        .await
        .expect("the run reports how it ended")
    }

    /// Whether process `pid` has exited within two seconds. A zombie counts
    /// as exited: an orphan is reaped by whichever process adopted it, which
    /// this test does not control.
    async fn gone(pid: u32) -> bool {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let output = Command::new("ps")
                    .args(["-o", "stat=", "-p", &pid.to_string()])
                    .output()
                    .await
                    .unwrap();
                let state = String::from_utf8_lossy(&output.stdout);
                if state.trim().is_empty() || state.trim_start().starts_with('Z') {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .is_ok()
    }

    /// The pid a script printed as the first line of its output.
    fn printed_pid(run: &Run) -> u32 {
        String::from_utf8(run.stdout())
            .unwrap()
            .lines()
            .next()
            .expect("the script prints a pid first")
            .parse()
            .unwrap()
    }

    async fn run(executor: &CommandExecutor, request: &InboundMessage) -> Run {
        let (sender, receiver) = mpsc::channel(100);
        executor.spawn(request, sender).await.unwrap();
        finish(receiver).await
    }

    fn shell(job_id: &str, script: &str) -> InboundMessage {
        InboundMessage::RunStart {
            job_id: job_id.to_string(),
            workspace: PathBuf::from("/tmp"),
            command: "sh".to_string(),
            args: vec!["-c".to_string(), script.to_string()],
            env: HashMap::new(),
            timeout_ms: Some(10_000),
            max_output_bytes: None,
            working_dir: None,
            confinement: None,
        }
    }

    /// Run `command` confined to `root`, which it may read and write.
    fn confined(job_id: &str, root: &Path, command: &str) -> InboundMessage {
        InboundMessage::RunStart {
            job_id: job_id.to_string(),
            workspace: root.to_path_buf(),
            command: command.to_string(),
            args: vec![],
            env: HashMap::new(),
            timeout_ms: Some(15_000),
            max_output_bytes: None,
            working_dir: None,
            confinement: Some(Box::new(ConfinementRequest {
                read_roots: vec![root.to_path_buf()],
                write_roots: vec![root.to_path_buf()],
                process_tree: None,
            })),
        }
    }

    fn limited(request: InboundMessage, limit: usize) -> InboundMessage {
        let InboundMessage::RunStart {
            job_id,
            workspace,
            command,
            args,
            env,
            timeout_ms,
            working_dir,
            confinement,
            ..
        } = request
        else {
            unreachable!("only a RunStart carries an output limit");
        };
        InboundMessage::RunStart {
            job_id,
            workspace,
            command,
            args,
            env,
            timeout_ms,
            max_output_bytes: Some(limit),
            working_dir,
            confinement,
        }
    }

    /// Re-run the test `name` in a child test process whose environment is
    /// `PATH` and [`REQUIRE_CONFINEMENT`] plus `environment`, so a test can
    /// shape the executor's own environment, or start with no sandbox verdict
    /// cached, without touching this process. Returns whether this call was
    /// the parent, which has nothing left to do once the child passes.
    async fn delegated_to_child(name: &str, environment: &[(&str, &str)]) -> bool {
        if std::env::var(CHILD).as_deref() == Ok(name) {
            return false;
        }
        let output = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", name, "--nocapture"])
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env(CHILD, name)
            .envs(std::env::var_os(REQUIRE_CONFINEMENT).map(|value| (REQUIRE_CONFINEMENT, value)))
            .envs(environment.iter().copied())
            .output()
            .await
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success(),
            "{stdout}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            stdout.contains("1 passed"),
            "the child ran no test, so it proved nothing\n{stdout}"
        );
        true
    }

    fn environment_listing(environment: HashMap<String, String>) -> InboundMessage {
        InboundMessage::RunStart {
            job_id: "environment".to_string(),
            workspace: std::env::temp_dir(),
            command: "env".to_string(),
            args: vec![],
            env: environment,
            working_dir: None,
            confinement: None,
            timeout_ms: Some(5000),
            max_output_bytes: None,
        }
    }

    async fn environment_of(executor: &CommandExecutor, request: &InboundMessage) -> String {
        let run = run(executor, request).await;
        assert_eq!(run.exit(), Some((Some(0), None)));
        String::from_utf8(run.stdout()).unwrap()
    }

    #[tokio::test]
    async fn the_default_policy_withholds_the_executor_environment() {
        const NAME: &str =
            "executor::command::tests::the_default_policy_withholds_the_executor_environment";
        const MARKER: &str = "ABNEGATE_EXEC_TEST_MASTER_KEY";
        const VALUE: &str = "hunter2";
        if delegated_to_child(NAME, &[(MARKER, VALUE), ("TERM", "xterm")]).await {
            return;
        }

        let output = environment_of(
            &CommandExecutor::new(),
            &environment_listing(HashMap::new()),
        )
        .await;

        assert!(!output.contains(MARKER), "{output}");
        assert!(!output.contains(VALUE), "{output}");
        assert!(
            output.lines().any(|line| line.starts_with("PATH=")),
            "{output}"
        );
        assert!(output.lines().any(|line| line == "TERM=xterm"), "{output}");
    }

    #[tokio::test]
    async fn inherit_passes_the_executor_environment_beneath_the_request() {
        const NAME: &str =
            "executor::command::tests::inherit_passes_the_executor_environment_beneath_the_request";
        const MARKER: &str = "ABNEGATE_EXEC_TEST_MASTER_KEY";
        const SHADOWED: &str = "ABNEGATE_EXEC_TEST_SHADOWED";
        if delegated_to_child(NAME, &[(MARKER, "hunter2"), (SHADOWED, "executor")]).await {
            return;
        }
        let executor = CommandExecutor::with_config(
            ExecutorConfig::default().with_environment(EnvironmentPolicy::Inherit),
        );

        let output = environment_of(
            &executor,
            &environment_listing(HashMap::from([(
                SHADOWED.to_string(),
                "request".to_string(),
            )])),
        )
        .await;

        assert!(
            output
                .lines()
                .any(|line| line == format!("{MARKER}=hunter2")),
            "{output}"
        );
        assert!(
            output
                .lines()
                .any(|line| line == format!("{SHADOWED}=request")),
            "{output}"
        );
    }

    #[tokio::test]
    async fn a_confined_run_layers_the_request_over_the_sandbox_over_the_policy() {
        const NAME: &str = "executor::command::tests::a_confined_run_layers_the_request_over_the_sandbox_over_the_policy";
        const MARKER: &str = "ABNEGATE_EXEC_TEST_MASTER_KEY";
        if delegated_to_child(
            NAME,
            &[
                (MARKER, "hunter2"),
                ("TERM", "xterm"),
                ("HOME", "/executor"),
            ],
        )
        .await
        {
            return;
        }
        if !sandbox::proven(ConfinementMode::SingleCommand).await {
            return;
        }
        let workspace = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(workspace.path()).unwrap();
        let mut request = confined("confined-environment", &root, "/usr/bin/env");
        if let InboundMessage::RunStart { env, .. } = &mut request {
            env.insert("LAYERED".to_string(), "request".to_string());
        }

        let output = environment_of(&CommandExecutor::new(), &request).await;
        let lines: Vec<&str> = output.lines().collect();

        assert!(lines.contains(&"LAYERED=request"), "{output}");
        assert!(lines.contains(&"TERM=xterm"), "{output}");
        assert!(
            lines.contains(&format!("HOME={}", root.display()).as_str()),
            "the sandbox's own HOME outranks the executor's: {output}"
        );
        assert!(!output.contains(MARKER), "{output}");
    }

    /// The first confined job waits for the sandbox to be proven, and none of
    /// that wait belongs to the job: its timeout and reported duration count
    /// from the spawn. Runs in a child process, where no verdict is cached.
    #[tokio::test]
    async fn the_first_confined_job_is_not_charged_for_proving_the_sandbox() {
        const NAME: &str = "executor::command::tests::the_first_confined_job_is_not_charged_for_proving_the_sandbox";
        if delegated_to_child(NAME, &[]).await {
            return;
        }
        let workspace = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(workspace.path()).unwrap();
        let (sender, receiver) = mpsc::channel(100);
        let before = Instant::now();

        let spawned = CommandExecutor::new()
            .spawn(&confined("first-confined", &root, "/usr/bin/true"), sender)
            .await;
        let spawning = before.elapsed();

        let handle = match spawned {
            Ok(handle) => handle,
            Err(ExecutorError::ConfinementUnavailable(error)) => {
                assert!(
                    !sandbox::required(ConfinementMode::SingleCommand),
                    "{REQUIRE_CONFINEMENT} is set, but this host cannot prove its sandbox: {error}"
                );
                return;
            }
            Err(error) => panic!("{error}"),
        };
        let run = finish(receiver).await;
        let clock = handle.started_at.duration_since(before);

        assert_eq!(run.exit(), Some((Some(0), None)), "{:?}", run.messages);
        assert!(
            clock >= spawning / 2,
            "the job's clock started {clock:?} into a {spawning:?} spawn, before the sandbox was proven"
        );
    }

    #[tokio::test]
    async fn proxy_overrides_request_environment() {
        const NAME: &str = "executor::command::tests::proxy_overrides_request_environment";
        if delegated_to_child(
            NAME,
            &[(crate::proxy::PROXY_URL_ENV, "http://127.0.0.1:28888")],
        )
        .await
        {
            return;
        }

        let output = environment_of(
            &CommandExecutor::new(),
            &environment_listing(HashMap::from([
                ("HTTPS_PROXY".to_string(), "http://wrong:8888".to_string()),
                ("http_proxy".to_string(), "http://wrong:8888".to_string()),
                ("NO_PROXY".to_string(), "*".to_string()),
                ("no_proxy".to_string(), "*".to_string()),
                (crate::proxy::PROXY_URL_ENV.to_string(), String::new()),
            ])),
        )
        .await;

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

    #[tokio::test]
    async fn a_non_utf8_byte_does_not_cut_the_stream() {
        let run = run(
            &CommandExecutor::new(),
            &shell("non-utf8", r"printf '\377'; seq 1 2000"),
        )
        .await;

        let stdout = run.stdout();
        assert_eq!(stdout.first(), Some(&0xFF));
        assert!(stdout.ends_with(b"1999\n2000\n"), "{} bytes", stdout.len());
        assert_eq!(
            run.exit(),
            Some((Some(0), None)),
            "the writer was killed by a closed pipe"
        );
    }

    #[tokio::test]
    async fn truncating_inside_a_character_does_not_panic() {
        let run = run(
            &CommandExecutor::new(),
            &limited(shell("split-character", r"printf 'a\303\251\n'"), 2),
        )
        .await;

        assert_eq!(run.stdout(), b"a\xC3");
        assert_eq!(run.exit(), Some((Some(0), None)));
        assert!(run.messages.iter().any(|message| matches!(
            message,
            OutboundMessage::RunLog { level: LogLevel::Warn, message, .. }
                if message == "Output truncated at 2 bytes"
        )));
    }

    #[tokio::test]
    async fn output_past_the_limit_is_drained_not_severed() {
        let run = run(
            &CommandExecutor::new(),
            &limited(shell("drained", "seq 1 200000"), 100),
        )
        .await;

        assert_eq!(run.stdout().len(), 100);
        assert_eq!(
            run.exit(),
            Some((Some(0), None)),
            "a writer past the limit must finish, not die of SIGPIPE"
        );
    }

    #[tokio::test]
    async fn stdout_and_stderr_share_one_limit() {
        let run = run(
            &CommandExecutor::new(),
            &limited(
                shell("shared-limit", "printf %0100d 0; printf %0100d 0 >&2"),
                100,
            ),
        )
        .await;

        assert_eq!(run.stdout().len() + run.stderr().len(), 100);
        assert_eq!(run.exit(), Some((Some(0), None)));
    }

    #[tokio::test]
    async fn output_without_a_newline_is_delivered_while_the_child_runs() {
        let executor = CommandExecutor::new();
        let (sender, mut receiver) = mpsc::channel(100);
        let handle = executor
            .spawn(&shell("partial-line", "printf prompt; sleep 30"), sender)
            .await
            .unwrap();

        let delivered = tokio::time::timeout(Duration::from_secs(5), async {
            while let Some(message) = receiver.recv().await {
                if let OutboundMessage::RunStdout { data, .. } = message {
                    return BASE64_STANDARD.decode(data).unwrap();
                }
            }
            Vec::new()
        })
        .await;
        handle.cancel();
        let run = finish(receiver).await;

        assert_eq!(
            delivered.expect("a line with no newline is held back until exit"),
            b"prompt"
        );
        assert_eq!(run.error(), Some(ErrorCode::Cancelled));
        assert!(gone(handle.pid).await);
    }

    #[tokio::test]
    async fn a_signalled_child_reports_its_signal() {
        let run = run(&CommandExecutor::new(), &shell("signalled", "kill -9 $$")).await;

        assert_eq!(run.exit(), Some((None, Some(9))));
    }

    #[tokio::test]
    async fn cancelling_stops_the_child_without_waiting_out_the_grace_period() {
        const GRACE: Duration = Duration::from_secs(5);
        let executor =
            CommandExecutor::with_config(ExecutorConfig::default().with_grace_period(GRACE));
        let (sender, receiver) = mpsc::channel(100);
        let cancellation = CancellationToken::new();
        let handle = executor
            .spawn_with_cancellation(
                &shell("cancelled", "sleep 30"),
                sender,
                cancellation.clone(),
            )
            .await
            .unwrap();
        let started = Instant::now();

        cancellation.cancel();
        let run = finish(receiver).await;

        assert!(handle.is_cancelled());
        assert_eq!(run.error(), Some(ErrorCode::Cancelled));
        assert!(
            started.elapsed() < GRACE,
            "a child that obeys SIGTERM is reported as soon as it exits, took {:?}",
            started.elapsed()
        );
        assert!(gone(handle.pid).await);
    }

    #[tokio::test]
    async fn a_timeout_kills_the_whole_group() {
        let executor = CommandExecutor::with_config(
            ExecutorConfig::default().with_grace_period(Duration::from_millis(100)),
        );
        let (sender, receiver) = mpsc::channel(100);
        let mut request = shell("timeout", "sleep 30 & echo $!; sleep 30");
        if let InboundMessage::RunStart { timeout_ms, .. } = &mut request {
            *timeout_ms = Some(200);
        }
        let handle = executor.spawn(&request, sender).await.unwrap();

        let run = finish(receiver).await;

        assert_eq!(run.error(), Some(ErrorCode::Timeout));
        assert!(gone(handle.pid).await);
        assert!(gone(printed_pid(&run)).await);
    }

    /// The consumer takes `RunStarted` and then stops reading, so every
    /// later message waits on it. Killing the child must not.
    #[tokio::test]
    async fn a_timeout_is_enforced_while_the_consumer_reads_nothing() {
        let executor = CommandExecutor::with_config(
            ExecutorConfig::default().with_grace_period(Duration::from_millis(100)),
        );
        let (sender, _receiver) = mpsc::channel(1);
        let mut request = shell("unread", "sleep 30");
        if let InboundMessage::RunStart { timeout_ms, .. } = &mut request {
            *timeout_ms = Some(200);
        }

        let handle = executor.spawn(&request, sender).await.unwrap();

        assert!(
            gone(handle.pid).await,
            "a consumer that stopped reading kept a timed-out child alive"
        );
    }

    /// The group is released just before its leader is reaped, so by the
    /// time any ending is reported no handle to it -- such as the one a
    /// registry holds -- can signal an identifier that is free for reuse.
    #[tokio::test]
    async fn every_ending_releases_the_group_before_it_is_reported() {
        let executor = CommandExecutor::with_config(
            ExecutorConfig::default().with_grace_period(Duration::from_millis(100)),
        );
        let mut timed_out = shell("released-timeout", "sleep 30");
        if let InboundMessage::RunStart { timeout_ms, .. } = &mut timed_out {
            *timeout_ms = Some(200);
        }
        let endings = [
            (shell("released-exit", "exit 0"), false),
            (timed_out, false),
            (shell("released-cancel", "sleep 30"), true),
        ];

        for (request, cancelled) in endings {
            let (sender, receiver) = mpsc::channel(100);
            let handle = executor.spawn(&request, sender).await.unwrap();
            if cancelled {
                handle.cancel();
            }
            let run = finish(receiver).await;

            assert!(
                handle.process_group.is_released(),
                "{:?}",
                run.messages.last()
            );
        }
    }

    #[tokio::test]
    async fn a_child_that_exits_takes_its_group_with_it() {
        let run = run(
            &CommandExecutor::new(),
            &shell("orphan", "sleep 60 > /dev/null 2>&1 & echo $!; exit 0"),
        )
        .await;

        assert_eq!(run.exit(), Some((Some(0), None)));
        assert!(
            gone(printed_pid(&run)).await,
            "a process the child left behind outlived the run"
        );
    }

    #[tokio::test]
    async fn a_descendant_holding_the_output_open_ends_with_the_child() {
        let started = Instant::now();

        let run = run(
            &CommandExecutor::new(),
            &shell("holder", "sleep 60 & echo $!; exit 0"),
        )
        .await;

        assert_eq!(run.exit(), Some((Some(0), None)));
        assert!(
            started.elapsed() < GRACE_PERIOD,
            "the run waited on a descendant, took {:?}",
            started.elapsed()
        );
        assert!(gone(printed_pid(&run)).await);
    }

    #[tokio::test]
    async fn cancelling_after_the_child_exits_still_ends_its_group() {
        let executor = CommandExecutor::new();
        let (sender, mut receiver) = mpsc::channel(100);
        let cancellation = CancellationToken::new();
        executor
            .spawn_with_cancellation(
                &shell("late-cancel", "sleep 60 & echo $!; exit 0"),
                sender,
                cancellation.clone(),
            )
            .await
            .unwrap();
        let mut printed = Vec::new();
        while !printed.contains(&b'\n') {
            match receiver.recv().await.expect("the run reports its output") {
                OutboundMessage::RunStdout { data, .. } => {
                    printed.extend(BASE64_STANDARD.decode(data).unwrap())
                }
                OutboundMessage::RunExit { .. } | OutboundMessage::RunError { .. } => break,
                _ => {}
            }
        }
        let background: u32 = String::from_utf8(printed).unwrap().trim().parse().unwrap();

        cancellation.cancel();
        finish(receiver).await;

        assert!(gone(background).await);
    }

    #[tokio::test]
    async fn a_zero_output_limit_still_warns() {
        let run = run(
            &CommandExecutor::new(),
            &limited(shell("silenced", "echo hello"), 0),
        )
        .await;

        assert!(run.stdout().is_empty());
        assert!(run.messages.iter().any(|message| matches!(
            message,
            OutboundMessage::RunLog { level: LogLevel::Warn, message, .. }
                if message == "Output truncated at 0 bytes"
        )));
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
        let (sender, receiver) = mpsc::channel(100);

        let handle = executor
            .spawn(&shell("cancel-test", "sleep 10"), sender)
            .await
            .unwrap();
        assert!(!handle.is_cancelled());

        handle.cancel();
        assert!(handle.is_cancelled());
        assert_eq!(finish(receiver).await.error(), Some(ErrorCode::Cancelled));
        assert!(gone(handle.pid).await);
    }

    #[tokio::test]
    async fn test_job_handle_elapsed() {
        let executor = CommandExecutor::new();
        let (sender, _receiver) = mpsc::channel(100);

        let handle = executor
            .spawn(&shell("elapsed-test", "echo test"), sender)
            .await
            .unwrap();

        tokio::time::sleep(Duration::from_millis(50)).await;

        assert!(handle.elapsed() >= Duration::from_millis(50));
    }

    #[tokio::test]
    async fn test_duration_spans_the_whole_child() {
        const SLEPT: Duration = Duration::from_millis(100);

        let run = run(
            &CommandExecutor::new(),
            &shell("duration-test", &format!("sleep {}", SLEPT.as_secs_f64())),
        )
        .await;

        let reported = run
            .messages
            .iter()
            .find_map(|message| match message {
                OutboundMessage::RunExit { duration_ms, .. } => Some(u128::from(*duration_ms)),
                _ => None,
            })
            .expect("the run reports how it ended");
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
        let (sender, mut receiver) = mpsc::channel(1);
        sender
            .send(OutboundMessage::log(
                "stall".to_string(),
                LogLevel::Info,
                "stall".to_string(),
                None,
            ))
            .await
            .unwrap();

        let drain = tokio::spawn(async move {
            tokio::time::sleep(STALL).await;
            while let Some(message) = receiver.recv().await {
                if let OutboundMessage::RunExit { duration_ms, .. } = message {
                    return Some(u128::from(duration_ms));
                }
            }
            None
        });

        executor
            .spawn(
                &shell(
                    "stalled-consumer-test",
                    &format!("sleep {}", SLEPT.as_secs_f64()),
                ),
                sender,
            )
            .await
            .unwrap();
        let reported = drain.await.unwrap().expect("the run reports how it ended");

        assert!(
            reported >= SLEPT.as_millis(),
            "a run that slept {}ms is reported as {reported}ms",
            SLEPT.as_millis()
        );
    }

    #[tokio::test]
    async fn test_spawn_echo() {
        let run = run(&CommandExecutor::new(), &shell("test-1", "echo hello")).await;

        assert!(matches!(
            run.messages.first(),
            Some(OutboundMessage::RunStarted { job_id, .. }) if job_id == "test-1"
        ));
        assert_eq!(run.stdout(), b"hello\n");
        assert_eq!(run.exit(), Some((Some(0), None)));
    }

    #[tokio::test]
    async fn test_spawn_with_working_dir() {
        let mut request = shell("workdir-test", "pwd");
        if let InboundMessage::RunStart { working_dir, .. } = &mut request {
            *working_dir = Some(PathBuf::from("/tmp"));
        }

        let run = run(&CommandExecutor::new(), &request).await;

        let output = String::from_utf8(run.stdout()).unwrap();
        assert!(output.contains("/tmp") || output.contains("/private/tmp"));
    }

    #[tokio::test]
    async fn test_spawn_with_env() {
        let mut request = shell("env-test", "echo $MY_VAR");
        if let InboundMessage::RunStart { env, .. } = &mut request {
            env.insert("MY_VAR".to_string(), "my_value".to_string());
        }

        let run = run(&CommandExecutor::new(), &request).await;

        assert_eq!(run.stdout(), b"my_value\n");
    }

    #[tokio::test]
    async fn test_spawn_stderr_output() {
        let run = run(
            &CommandExecutor::new(),
            &shell("stderr-test", "echo error >&2"),
        )
        .await;

        assert_eq!(
            run.stderr(),
            b"error\n",
            "stderr must be delivered before RunExit"
        );
    }

    #[tokio::test]
    async fn test_invalid_workspace() {
        let executor = CommandExecutor::new();
        let (sender, _receiver) = mpsc::channel(100);
        let mut request = shell("test-2", "true");
        if let InboundMessage::RunStart { workspace, .. } = &mut request {
            *workspace = PathBuf::from("/nonexistent/path");
        }

        match executor.spawn(&request, sender).await {
            Err(ExecutorError::InvalidWorkspace(message)) => {
                assert!(message.contains("does not exist"));
            }
            other => panic!("Wrong result: {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_spawn_workspace_is_file() {
        let executor = CommandExecutor::new();
        let (sender, _receiver) = mpsc::channel(100);
        let mut request = shell("file-workspace-test", "true");
        if let InboundMessage::RunStart { workspace, .. } = &mut request {
            *workspace = PathBuf::from("/etc/passwd");
        }

        match executor.spawn(&request, sender).await {
            Err(ExecutorError::InvalidWorkspace(message)) => {
                assert!(message.contains("not a directory"));
            }
            other => panic!("Wrong result: {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_message_other_than_run_start_is_an_invalid_message() {
        let executor = CommandExecutor::new();
        let (sender, _receiver) = mpsc::channel(100);

        let result = executor
            .spawn(
                &InboundMessage::Ping {
                    id: "1".to_string(),
                },
                sender,
            )
            .await;

        match result {
            Err(error @ ExecutorError::NotRunStart) => {
                assert_eq!(error.to_error_code(), ErrorCode::InvalidMessage);
            }
            other => panic!("Wrong result: {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_spawn_non_zero_exit() {
        let run = run(&CommandExecutor::new(), &shell("nonzero-test", "exit 42")).await;

        assert_eq!(run.exit(), Some((Some(42), None)));
    }

    #[tokio::test]
    async fn test_spawn_invalid_command() {
        let executor = CommandExecutor::new();
        let (sender, _receiver) = mpsc::channel(100);
        let mut request = shell("invalid-command-test", "");
        if let InboundMessage::RunStart { command, .. } = &mut request {
            *command = "/nonexistent/binary/that/doesnt/exist".to_string();
        }

        match executor.spawn(&request, sender).await {
            Err(ExecutorError::SpawnFailed(_)) => {}
            other => panic!("Wrong result: {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_spawn_with_custom_output_limit() {
        let run = run(
            &CommandExecutor::new(),
            &limited(
                shell("limit-test", "for i in $(seq 1 100); do echo line$i; done"),
                100,
            ),
        )
        .await;

        assert_eq!(run.stdout().len(), 100);
    }
}
