//! A provider backed by a coding agent CLI run as a child process.

use std::io::Write;
use std::path::PathBuf;
use std::process::Stdio;

use abnegate_exec::executor::GRACE_PERIOD;
use abnegate_exec::executor::ProcessGroup;
use abnegate_llm::Capabilities;
use abnegate_llm::Completion;
use abnegate_llm::CompletionProvider;
use abnegate_llm::CompletionRequest;
use abnegate_llm::ExitStatus;
use abnegate_llm::Message;
use abnegate_llm::ProviderError;
use abnegate_llm::ProviderKind;
use async_trait::async_trait;
use serde_json::json;
use tempfile::NamedTempFile;
use tokio::io::AsyncWriteExt;
use tokio::process::Child;
use tokio::process::Command;
use tokio::sync::mpsc;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tokio::time::timeout;
use tokio::time::timeout_at;

use crate::attachments::Attachments;
use crate::diagnostics::Diagnostics;
use crate::environment::Environment;
use crate::execution::Execution;
use crate::execution_error::ExecutionError;
use crate::kind::AgentKind;
use crate::log::EXECUTION_LOG_PREVIEW_LIMIT;
use crate::log::ExecutionLogFiles;
use crate::log::Journal;
use crate::log::Record;
use crate::log::Sink;
use crate::log::preview;
use crate::mcp::McpAttachment;
use crate::outcome::Outcome;
use crate::reader::Reader;
use crate::reaper::Reaper;
use crate::scrubber::Scrubber;
use crate::settings::CliSettings;
use crate::transcript;
use crate::verdict::Verdict;

const NO_DIAGNOSTICS: &str = "the agent produced no diagnostics";
const UNFINISHED: &str = "the agent exited without completing its event stream";
const UNCLOSED: &str = "the agent's output stayed open after it exited";
const UNSTOPPABLE: &str = "the agent could not be stopped";
const LINGERED: &str = "the agent finished its turn but did not exit";
const INSTRUCTIONS_PREFIX: &str = "instructions-";
const INSTRUCTIONS_SUFFIX: &str = ".md";

/// Drives a coding agent CLI as a completion provider.
///
/// The agent runs its own tool loop, so [`CompletionRequest::tools`] is not
/// forwarded: the agent has its own tools and no way to call the caller's.
/// What comes back is the agent's final prose. Tool activity is still parsed,
/// so a stream full of it cannot derail the run, but it is not returned as
/// tool calls: the agent already executed them, and replaying them through the
/// caller's registry would run each one a second time.
///
/// # Confinement
///
/// The agent does not run under `abnegate-exec`'s sandbox. That sandbox denies
/// all network access and grants `process-exec` for a single literal command,
/// and a coding agent needs the network to reach its own API and forks a tree
/// of helper processes to do its work. It does reuse that crate's
/// process-group termination and output caps, so a run that times out or
/// fails takes the agent's whole process tree with it, and it is given only
/// the [`DEFAULT_ENVIRONMENT`](abnegate_exec::DEFAULT_ENVIRONMENT) names and
/// those [`CliSettings::allow`] adds from this process's environment unless
/// [`CliSettings::inherit_environment`] opts in.
#[derive(Debug)]
pub struct CliProvider {
    name: String,
    agent: AgentKind,
    settings: CliSettings,
}

impl CliProvider {
    pub fn new(name: impl Into<String>, agent: AgentKind, settings: CliSettings) -> Self {
        Self {
            name: name.into(),
            agent,
            settings,
        }
    }

    /// A provider named after the agent it drives.
    pub fn agent(agent: AgentKind, settings: CliSettings) -> Self {
        Self::new(agent.as_str(), agent, settings)
    }

    pub fn settings(&self) -> &CliSettings {
        &self.settings
    }

    /// Run the agent once and report everything it did, with `label` naming
    /// the run in its execution logs.
    ///
    /// A run that exits on its own, or is stopped once its output settled it,
    /// is an [`Execution`] however it ended; judging it is left to the caller,
    /// as [`CompletionProvider::complete`] does. What fails here is only what
    /// leaves nothing to judge: settings the agent cannot honour, an agent
    /// that cannot be started, one that outlives its timeout, and output that
    /// cannot be read as the agent's stream.
    ///
    /// Only a run that succeeded leaves behind what the agent forked. Any
    /// other takes the agent's whole process group with it. A run that fails
    /// here still reports where its logs are, with every line read before it
    /// failed flushed to them.
    pub async fn execute(
        &self,
        request: CompletionRequest<'_>,
        label: &str,
    ) -> Result<Execution, ExecutionError> {
        let mcp = self.attach();
        let instructions = self
            .instructions()
            .map_err(|error| ExecutionError::new(error, None))?;
        let mut attachments = Attachments::default();
        if let Some(mcp) = &mcp {
            attachments = attachments.with_mcp(mcp.file.path());
        }
        if let Some(instructions) = &instructions {
            attachments = attachments.with_instructions(instructions.path());
        }
        let options = self
            .agent
            .options(&self.settings, &attachments)
            .map_err(|error| ExecutionError::new(error, None))?;
        let arguments = self.agent.invocation(Some(request.model), options);
        let environment = Environment::new(self.agent, &self.settings, mcp.as_ref(), &|name| {
            std::env::var_os(name)
        });
        let scrubber = Scrubber::new(environment.secrets());

        let files = self
            .settings
            .log
            .as_deref()
            .and_then(|root| ExecutionLogFiles::create(root, self.agent.as_str(), label));
        let journal = match &files {
            Some(files) => Journal::open(&files.events, label, self.settings.journal_limit).await,
            None => Journal::disabled(),
        };
        let logged: Vec<_> = arguments
            .iter()
            .map(|argument| scrubber.scrub(argument))
            .collect();
        journal
            .append(
                Record::Initialized,
                json!({
                    "provider": self.name,
                    "agent": self.agent.as_str(),
                    "model": request.model,
                    "timeout_seconds": self.settings.timeout.as_secs(),
                    "working_directory": self.settings.working_directory,
                    "arguments": logged,
                }),
            )
            .await;
        tracing::info!(
            provider = %self.name,
            agent = %self.agent,
            label,
            timeout_seconds = self.settings.timeout.as_secs(),
            "starting agent"
        );

        let mut child = match self.command(&arguments, &environment).spawn() {
            Ok(child) => child,
            Err(error) => {
                journal
                    .append(Record::SpawnFailed, json!({ "error": error.to_string() }))
                    .await;
                return Err(ExecutionError::new(
                    ProviderError::unavailable(&self.name, &self.executable(), error),
                    files,
                ));
            }
        };
        let reaper = Reaper::new(child.id().and_then(|pid| ProcessGroup::try_from(pid).ok()));
        journal
            .append(Record::Spawned, json!({ "pid": child.id() }))
            .await;

        // The prompt is written concurrently with reading the reply: a prompt
        // larger than the pipe buffer blocks until the child drains it, and a
        // child nobody reads from blocks on its own output first.
        let prompt = transcript::render(request.messages);
        let stdin = child.stdin.take();
        let writer = tokio::spawn(async move {
            if let Some(mut stdin) = stdin {
                let _ = stdin.write_all(prompt.as_bytes()).await;
                let _ = stdin.shutdown().await;
            }
        });

        let (verdicts, mut settled) = mpsc::channel(2);
        let (cancel, cancelled) = watch::channel(false);
        let prose = Sink::open(files.as_ref().map(|files| files.stdout.as_path())).await;
        let raw = Sink::open(files.as_ref().map(|files| files.stderr.as_path())).await;
        let mut reader = tokio::spawn(
            Reader::new(
                self.agent,
                self.settings.line_limit,
                self.settings.output_limit,
                journal.clone(),
                prose,
                scrubber.clone(),
                verdicts.clone(),
                cancelled.clone(),
            )
            .run(child.stdout.take()),
        );
        let mut diagnostics = tokio::spawn(
            Diagnostics::new(
                self.settings.line_limit,
                self.settings.output_limit,
                journal.clone(),
                raw,
                scrubber.clone(),
                self.settings.tripwire.clone(),
                verdicts,
                cancelled,
            )
            .run(child.stderr.take()),
        );

        let deadline = Instant::now().checked_add(self.settings.timeout);
        let expiry = async {
            match deadline {
                Some(deadline) => tokio::time::sleep_until(deadline).await,
                None => std::future::pending().await,
            }
        };
        let outcome = tokio::select! {
            biased;
            status = child.wait() => Outcome::Exited(status),
            Some(verdict) = settled.recv() => Outcome::Settled(verdict),
            () = expiry => Outcome::TimedOut,
        };

        let (status, stopped, failure) = match self
            .settle(
                outcome,
                &mut child,
                reaper.group(),
                &journal,
                deadline,
                label,
            )
            .await
        {
            Ok(settlement) => settlement,
            Err(error) => {
                writer.abort();
                let _ = drain(&mut reader, reaper.group(), &cancel).await;
                let _ = drain(&mut diagnostics, reaper.group(), &cancel).await;
                return Err(ExecutionError::new(error, files));
            }
        };
        let status = ExitStatus::from(status);
        journal
            .append(Record::Exited, json!({ "status": status.to_string() }))
            .await;

        writer.abort();
        let stdout = match drain(&mut reader, reaper.group(), &cancel).await {
            Ok(Ok(stdout)) => stdout,
            Ok(Err(message)) | Err(message) => {
                let _ = drain(&mut diagnostics, reaper.group(), &cancel).await;
                return Err(ExecutionError::new(
                    ProviderError::malformed(&self.name, message),
                    files,
                ));
            }
        };
        let stderr = drain(&mut diagnostics, reaper.group(), &cancel)
            .await
            .unwrap_or_default();
        let failure = failure.or_else(|| {
            std::iter::from_fn(|| settled.try_recv().ok()).find_map(|verdict| match verdict {
                Verdict::Failed(reason) => Some(reason),
                Verdict::Finished => None,
            })
        });
        if status == ExitStatus::Code(0)
            && stdout.finished
            && stdout.failure.is_none()
            && stopped.is_none()
        {
            reaper.disarm();
        }

        let reported = stdout
            .failure
            .as_deref()
            .or(failure.as_deref())
            .map(|reported| preview(reported, EXECUTION_LOG_PREVIEW_LIMIT));
        journal
            .append(
                Record::Completed,
                json!({
                    "status": status.to_string(),
                    "stdout_bytes": stdout.text.len(),
                    "stderr_bytes": stderr.len(),
                    "finished": stdout.finished,
                    "has_structured_result": stdout.structured.is_some(),
                    "failure": reported,
                    "stopped": stopped,
                }),
            )
            .await;

        Ok(Execution {
            stdout,
            stderr,
            status,
            stopped,
            failure,
            log: files,
        })
    }

    /// Carry the wait through to an exit status, stopping the agent when the
    /// wait ended without one, and say why it was stopped when it was and
    /// what failure settled the run when one did.
    async fn settle(
        &self,
        outcome: Outcome,
        child: &mut Child,
        group: Option<&ProcessGroup>,
        journal: &Journal,
        deadline: Option<Instant>,
        label: &str,
    ) -> Result<(std::process::ExitStatus, Option<String>, Option<String>), ProviderError> {
        let verdict = match outcome {
            Outcome::Exited(Ok(status)) => return Ok((status, None, None)),
            Outcome::Exited(Err(error)) => {
                journal
                    .append(Record::WaitFailed, json!({ "error": error.to_string() }))
                    .await;
                stop_agent(child, group).await;
                return Err(ProviderError::malformed(&self.name, error));
            }
            Outcome::TimedOut => {
                journal
                    .append(
                        Record::TimedOut,
                        json!({ "timeout_seconds": self.settings.timeout.as_secs() }),
                    )
                    .await;
                tracing::warn!(provider = %self.name, label, "agent timed out; stopping its process group");
                stop_agent(child, group).await;
                return Err(ProviderError::Timeout {
                    provider: self.name.clone(),
                    seconds: self.settings.timeout.as_secs(),
                });
            }
            Outcome::Settled(verdict) => verdict,
        };

        let (reason, failure) = match verdict {
            Verdict::Finished => (LINGERED.to_string(), None),
            Verdict::Failed(reason) => {
                let reason = preview(&reason, EXECUTION_LOG_PREVIEW_LIMIT);
                journal
                    .append(Record::Abandoned, json!({ "reason": reason }))
                    .await;
                tracing::warn!(provider = %self.name, label, "the run failed while the agent was running; stopping it");
                (reason.clone(), Some(reason))
            }
        };

        let grace = Instant::now() + GRACE_PERIOD;
        let patience = deadline.map_or(grace, |deadline| deadline.min(grace));
        if let Ok(Ok(status)) = timeout_at(patience, child.wait()).await {
            return Ok((status, None, failure));
        }

        let status = stop_agent(child, group)
            .await
            .ok_or_else(|| ProviderError::malformed(&self.name, UNSTOPPABLE))?;
        journal
            .append(
                Record::Stopped,
                json!({ "exit_code": status.code(), "reason": reason }),
            )
            .await;
        Ok((status, Some(reason), failure))
    }

    fn assemble(&self, execution: Execution) -> Result<Completion, ProviderError> {
        let Execution {
            stdout,
            stderr,
            status,
            stopped,
            failure,
            ..
        } = execution;

        // The agent's own report of what went wrong beats an exit code, which
        // says only that something did.
        if let Some(message) = &stdout.failure {
            return Err(ProviderError::agent(&self.name, message));
        }

        let unstopped = stopped.is_none();
        match (failure.or(stopped), stdout.finished) {
            (Some(reason), false) => return Err(ProviderError::agent(&self.name, &reason)),
            _ if unstopped && status != ExitStatus::Code(0) => {
                let message = if stderr.trim().is_empty() {
                    NO_DIAGNOSTICS.to_string()
                } else {
                    preview(&stderr, EXECUTION_LOG_PREVIEW_LIMIT)
                };
                return Err(ProviderError::exit(&self.name, status, &message));
            }
            // A clean exit is not proof of a finished turn. An agent killed
            // between its last token and its result line still exits zero.
            (None, false) => return Err(ProviderError::agent(&self.name, UNFINISHED)),
            (_, true) => {}
        }

        Ok(Completion::new(&self.name, Message::assistant(stdout.text))
            .with_usage(stdout.usage)
            .with_finish_reason(stdout.finish_reason))
    }

    /// Render the MCP servers to attach, or attach none when rendering fails:
    /// a run without its MCP tools can still answer, and fails loudly on its
    /// own if it truly needed them.
    fn attach(&self) -> Option<McpAttachment> {
        let mcp = &self.settings.mcp;
        if mcp.is_empty() || self.agent != AgentKind::Claude {
            return None;
        }
        match mcp.render() {
            Ok(Some(attachment)) => {
                tracing::info!(
                    provider = %self.name,
                    servers = mcp.attachable().count(),
                    "attaching MCP servers"
                );
                tracing::debug!(
                    provider = %self.name,
                    path = %attachment.file.path().display(),
                    config = %mcp.redacted(),
                    "rendered MCP config, secret values redacted"
                );
                Some(attachment)
            }
            Ok(None) => None,
            Err(error) => {
                tracing::warn!(
                    provider = %self.name,
                    %error,
                    "could not render the MCP config; continuing without MCP servers"
                );
                None
            }
        }
    }

    /// Write the instructions to a private temporary file for the agent to
    /// read, since `argv` has a hard size limit that instructions can reach.
    /// The file is deleted when the returned handle drops.
    fn instructions(&self) -> Result<Option<NamedTempFile>, ProviderError> {
        let Some(instructions) = &self.settings.instructions else {
            return Ok(None);
        };
        if self.agent != AgentKind::Claude {
            return Ok(None);
        }
        let write = || -> std::io::Result<NamedTempFile> {
            let mut file = tempfile::Builder::new()
                .prefix(INSTRUCTIONS_PREFIX)
                .suffix(INSTRUCTIONS_SUFFIX)
                .tempfile()?;
            file.as_file_mut().write_all(instructions.as_bytes())?;
            file.as_file_mut().flush()?;
            Ok(file)
        };
        write().map(Some).map_err(|error| {
            ProviderError::io(format!("could not write the instructions: {error}"))
        })
    }

    fn executable(&self) -> String {
        self.settings.executable.as_ref().map_or_else(
            || self.agent.executable().to_string(),
            |path| path.display().to_string(),
        )
    }

    fn command(&self, arguments: &[String], environment: &Environment) -> Command {
        let executable = self
            .settings
            .executable
            .clone()
            .unwrap_or_else(|| PathBuf::from(self.agent.executable()));

        let mut command = Command::new(executable);
        command
            .args(arguments)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        if let Some(directory) = &self.settings.working_directory {
            command.current_dir(directory);
        }
        environment.apply(&mut command);

        // The agent leads its own group so that stopping the run reaches the
        // language servers, searches and builds it forked, not just itself.
        command.process_group(0);
        command
    }
}

#[async_trait]
impl CompletionProvider for CliProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn kind(&self) -> ProviderKind {
        ProviderKind::Cli
    }

    fn capabilities(&self) -> Capabilities {
        self.agent.capabilities()
    }

    async fn complete(&self, request: CompletionRequest<'_>) -> Result<Completion, ProviderError> {
        let execution = self.execute(request, &self.name).await?;
        self.assemble(execution)
    }
}

/// Terminate the agent's group, and kill whatever is left of it once the
/// grace period runs out.
///
/// A group is identified by its leader's process id, which the system may
/// hand to a new process once the leader is reaped and no member is left.
/// So the group is killed while the leader is still unreaped whenever it
/// outlives the grace period. When the leader exits within it, reaping comes
/// first, and the kill that follows reaches stragglers safely only because a
/// straggler still alive keeps the group's id from being reused; a group
/// that empties in the instant between the two leaves the kill to land on
/// whatever took the id, a race this cannot close without a process handle
/// the platform does not offer here.
async fn stop_agent(
    child: &mut Child,
    group: Option<&ProcessGroup>,
) -> Option<std::process::ExitStatus> {
    let Some(group) = group else {
        let _ = child.start_kill();
        return child.wait().await.ok();
    };
    let _ = group.terminate();
    if let Ok(Ok(status)) = timeout(GRACE_PERIOD, child.wait()).await {
        let _ = group.kill();
        return Some(status);
    }
    let _ = group.kill();
    child.wait().await.ok()
}

/// A reader's result once the agent has exited.
///
/// A descendant that outlived the agent can still hold its output open, and a
/// reader waiting for that end of file would wait forever. Such stragglers get
/// the grace period to finish, then the rest of the group is killed, and a
/// reader still held open by something outside the group is told to stop with
/// what it has.
async fn drain<T>(
    task: &mut JoinHandle<T>,
    group: Option<&ProcessGroup>,
    cancel: &watch::Sender<bool>,
) -> Result<T, String> {
    if let Ok(joined) = timeout(GRACE_PERIOD, &mut *task).await {
        return joined.map_err(|error| error.to_string());
    }
    if let Some(group) = group {
        let _ = group.kill();
    }
    if let Ok(joined) = timeout(GRACE_PERIOD, &mut *task).await {
        return joined.map_err(|error| error.to_string());
    }
    tracing::warn!("{UNCLOSED}; keeping what was read");
    let _ = cancel.send(true);
    task.await.map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use std::path::PathBuf;
    use std::process::Stdio;
    use std::time::Duration;
    use std::time::Instant;

    use abnegate_exec::DEFAULT_ENVIRONMENT;
    use abnegate_llm::Completion;
    use abnegate_llm::CompletionProvider;
    use abnegate_llm::CompletionRequest;
    use abnegate_llm::Credential;
    use abnegate_llm::ExitStatus;
    use abnegate_llm::Message;
    use abnegate_llm::ProviderError;
    use abnegate_llm::ProviderKind;
    use abnegate_llm::RequestOptions;
    use abnegate_secret::SecretValue;
    use serde_json::Value;
    use tempfile::TempDir;

    use super::CliProvider;
    use crate::execution::Execution;
    use crate::kind::AgentKind;
    use crate::mcp::McpServer;
    use crate::settings::CliSettings;
    use crate::structured_result::StructuredResult;

    const ETXTBSY: i32 = 26;
    const PROBE: &str = "FAKE_AGENT_PROBE";

    /// A stand-in agent, so no test needs a real CLI installed.
    ///
    /// It reads its prompt from stdin exactly as the real agents do, which is
    /// what keeps the delivery path under test the real one. Started with
    /// [`PROBE`] set, it exits at once, so probing it never leaves a
    /// long-running script behind.
    fn fake(directory: &TempDir, script: &str) -> PathBuf {
        let path = directory.path().join("agent");
        let mut file = std::fs::File::create(&path).expect("the fake agent");
        write!(file, "#!/bin/sh\n[ -n \"${PROBE}\" ] && exit 0\n{script}\n")
            .expect("the fake agent body");
        drop(file);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .expect("the fake agent to be executable");
        wait_until_executable(&path);
        path
    }

    /// Linux refuses to exec a file any process still holds open for writing.
    /// The descriptor here is closed, but a sibling test forking between its
    /// own open and exec inherits it for that window, so a freshly written
    /// script can hit ETXTBSY under a parallel run. Production never meets
    /// this: a provider execs an installed binary, not one it just wrote.
    fn wait_until_executable(path: &Path) {
        for _ in 0..50 {
            match std::process::Command::new(path)
                .env(PROBE, "1")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
            {
                Ok(mut child) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return;
                }
                Err(error) if error.raw_os_error() == Some(ETXTBSY) => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(_) => return,
            }
        }
    }

    fn alive(pid: &str) -> bool {
        std::process::Command::new("kill")
            .args(["-0", pid])
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }

    fn settings(directory: &TempDir, script: &str) -> CliSettings {
        CliSettings::default()
            .with_executable(fake(directory, script))
            .with_timeout(Duration::from_secs(20))
    }

    fn request(messages: &[Message]) -> CompletionRequest<'_> {
        CompletionRequest::new("sonnet", messages, RequestOptions::new(512))
    }

    async fn run(
        provider: &CliProvider,
        messages: &[Message],
    ) -> Result<Completion, ProviderError> {
        provider.complete(request(messages)).await
    }

    async fn execute(provider: &CliProvider, messages: &[Message]) -> Execution {
        provider
            .execute(request(messages), "test-run")
            .await
            .expect("an execution")
    }

    const CLAUDE_SESSION: &str = r#"
echo '{"type":"system","subtype":"init","session_id":"6f1"}'
echo '{"type":"assistant","message":{"content":[{"type":"text","text":"Looking now. "}],"usage":{"input_tokens":4,"cache_read_input_tokens":800,"output_tokens":6}}}'
echo '{"type":"assistant","message":{"content":[{"type":"tool_use","id":"toolu_01","name":"Read","input":{"file_path":"/w/a.rs"}}]}}'
echo '{"type":"assistant","message":{"content":[{"type":"text","text":"It is empty."}]}}'
echo 'warming up the model' >&2
echo '{"type":"result","subtype":"success","is_error":false,"total_cost_usd":0.0412,"num_turns":2,"duration_api_ms":7980,"session_id":"6f1","structured_output":{"summary":"Read a.rs","success":true,"confidence":90},"usage":{"input_tokens":9,"cache_read_input_tokens":1600,"output_tokens":24}}'
"#;

    #[tokio::test]
    async fn a_streamed_session_becomes_one_assistant_message() {
        let directory = TempDir::new().expect("a temporary directory");
        let provider = CliProvider::agent(AgentKind::Claude, settings(&directory, CLAUDE_SESSION));

        let completion = run(&provider, &[Message::user("What does a.rs do?")])
            .await
            .expect("an answer");

        assert_eq!(completion.provider, "claude");
        assert_eq!(
            completion.message.content.as_deref(),
            Some("Looking now. It is empty.")
        );
        assert_eq!(completion.finish_reason.as_deref(), Some("success"));

        let usage = completion.usage.expect("token counts");
        assert_eq!(usage.prompt_tokens, 9 + 1600);
        assert_eq!(usage.completion_tokens, 24);
    }

    #[tokio::test]
    async fn an_execution_reports_everything_the_agent_said_about_the_run() {
        let directory = TempDir::new().expect("a temporary directory");
        let provider = CliProvider::agent(AgentKind::Claude, settings(&directory, CLAUDE_SESSION));

        let execution = execute(&provider, &[Message::user("What does a.rs do?")]).await;

        assert_eq!(execution.status, ExitStatus::Code(0));
        assert!(execution.log.is_none());
        assert!(execution.stderr.contains("warming up the model"));

        let stdout = execution.stdout;
        assert_eq!(stdout.session.as_deref(), Some("6f1"));
        assert!((stdout.cost.expect("a cost") - 0.0412).abs() < 1e-9);
        assert_eq!(stdout.turns, Some(2));
        assert_eq!(stdout.latency, Some(Duration::from_millis(7980)));
        assert_eq!(
            stdout
                .tokens
                .as_ref()
                .and_then(|tokens| tokens.cache_read_input_tokens),
            Some(1600)
        );
        assert_eq!(stdout.tools.len(), 1);
        assert_eq!(stdout.tools[0].function.name, "Read");

        let report: StructuredResult = stdout.decode().expect("a structured report");
        assert!(report.success);
        assert_eq!(report.summary, "Read a.rs");
        assert_eq!(report.confidence, 90);
    }

    #[tokio::test]
    async fn the_prompt_reaches_the_agent_on_stdin() {
        let directory = TempDir::new().expect("a temporary directory");
        let script = r#"
prompt=$(cat | tr '\n' ' ')
printf '{"type":"assistant","message":{"content":[{"type":"text","text":"%s"}]}}\n' "$prompt"
echo '{"type":"result","subtype":"success","is_error":false}'
"#;
        let provider = CliProvider::agent(AgentKind::Claude, settings(&directory, script));

        let completion = run(&provider, &[Message::user("ping")])
            .await
            .expect("an answer");

        let content = completion.message.content.unwrap_or_default();
        assert!(content.contains("User:"), "roles were lost: {content}");
        assert!(content.contains("ping"), "the prompt was lost: {content}");
    }

    #[tokio::test]
    async fn a_prompt_far_larger_than_a_pipe_buffer_still_gets_through() {
        let directory = TempDir::new().expect("a temporary directory");
        let script = r#"
bytes=$(cat | wc -c | tr -d ' ')
printf '{"type":"assistant","message":{"content":[{"type":"text","text":"%s"}]}}\n' "$bytes"
echo '{"type":"result","subtype":"success","is_error":false}'
"#;
        let provider = CliProvider::agent(AgentKind::Claude, settings(&directory, script));
        let large = "x".repeat(512 * 1024);
        let messages = [Message::user(large.clone())];

        let completion = run(&provider, &messages).await.expect("an answer");

        let reported: usize = completion
            .message
            .content
            .as_deref()
            .expect("a byte count")
            .trim()
            .parse()
            .expect("a number");
        assert!(
            reported >= large.len(),
            "the agent received only {reported} of {} bytes",
            large.len()
        );
    }

    #[tokio::test]
    async fn an_agent_that_reports_a_failure_fails_the_request() {
        let directory = TempDir::new().expect("a temporary directory");
        let script = r#"echo '{"type":"result","subtype":"error_during_execution","is_error":true,"result":"Invalid API key provided"}'"#;
        let provider = CliProvider::agent(AgentKind::Claude, settings(&directory, script));

        let error = run(&provider, &[Message::user("hi")])
            .await
            .expect_err("a failure");

        assert!(
            error.to_string().contains("Invalid API key"),
            "lost the agent's wording: {error}"
        );
        assert_eq!(error.provider(), Some("claude"));
    }

    #[tokio::test]
    async fn a_reported_failure_is_still_an_execution_for_the_caller_to_judge() {
        let directory = TempDir::new().expect("a temporary directory");
        let script = r#"echo '{"type":"result","subtype":"error_max_turns","is_error":true,"result":"","session_id":"s-9"}'"#;
        let provider = CliProvider::agent(AgentKind::Claude, settings(&directory, script));

        let execution = execute(&provider, &[Message::user("hi")]).await;

        assert_eq!(execution.stdout.failure.as_deref(), Some("error_max_turns"));
        assert_eq!(execution.stdout.session.as_deref(), Some("s-9"));
        assert!(execution.stdout.finished);
    }

    #[tokio::test]
    async fn a_rate_limited_agent_is_stopped_rather_than_waited_on() {
        let directory = TempDir::new().expect("a temporary directory");
        let script = r#"
echo '{"type":"rate_limit_event","rate_limit_info":{"status":"rejected","rateLimitType":"five_hour","resetsAt":1772096400}}'
sleep 120
"#;
        let provider = CliProvider::agent(
            AgentKind::Claude,
            settings(&directory, script).with_timeout(Duration::from_secs(60)),
        );

        let started = Instant::now();
        let error = run(&provider, &[Message::user("hi")])
            .await
            .expect_err("a failure");

        assert!(
            started.elapsed() < Duration::from_secs(30),
            "waited {:?} on a throttled agent",
            started.elapsed()
        );
        let ProviderError::Agent { message, .. } = &error else {
            panic!("expected the agent's own failure, got {error:?}");
        };
        assert!(message.contains("rate limit reached"), "{message}");
        assert!(message.contains("five_hour"), "{message}");
        assert!(message.contains(r#""resetsAt":1772096400"#), "{message}");
    }

    #[tokio::test]
    async fn a_nonzero_exit_reports_the_agents_diagnostics() {
        let directory = TempDir::new().expect("a temporary directory");
        let script = "echo 'error: could not reach the model endpoint' >&2\nexit 3";
        let provider = CliProvider::agent(AgentKind::Claude, settings(&directory, script));

        let error = run(&provider, &[Message::user("hi")])
            .await
            .expect_err("a failure");

        let ProviderError::Exit {
            status, message, ..
        } = &error
        else {
            panic!("expected an exit failure, got {error:?}");
        };
        assert_eq!(*status, ExitStatus::Code(3));
        assert!(message.contains("could not reach the model endpoint"));
    }

    #[tokio::test]
    async fn a_silent_nonzero_exit_says_there_were_no_diagnostics() {
        let directory = TempDir::new().expect("a temporary directory");
        let provider = CliProvider::agent(AgentKind::Claude, settings(&directory, "exit 7"));

        let error = run(&provider, &[Message::user("hi")])
            .await
            .expect_err("a failure");

        assert!(
            matches!(&error, ProviderError::Exit { status: ExitStatus::Code(7), message, .. } if message.contains("no diagnostics")),
            "{error:?}"
        );
    }

    #[tokio::test]
    async fn a_clean_exit_without_a_result_is_not_treated_as_an_answer() {
        let directory = TempDir::new().expect("a temporary directory");
        let script = r#"echo '{"type":"assistant","message":{"content":[{"type":"text","text":"half an ans"}]}}'"#;
        let provider = CliProvider::agent(AgentKind::Claude, settings(&directory, script));

        let error = run(&provider, &[Message::user("hi")])
            .await
            .expect_err("a failure");

        assert!(
            error.to_string().contains("without completing"),
            "truncation was hidden: {error}"
        );
    }

    #[tokio::test]
    async fn an_agent_that_never_finishes_is_stopped_at_the_timeout() {
        let directory = TempDir::new().expect("a temporary directory");
        let settings = CliSettings::default()
            .with_executable(fake(&directory, "sleep 120"))
            .with_timeout(Duration::from_millis(300));
        let provider = CliProvider::agent(AgentKind::Claude, settings);

        let started = Instant::now();
        let error = run(&provider, &[Message::user("hi")])
            .await
            .expect_err("a failure");

        assert!(
            matches!(error, ProviderError::Timeout { .. }),
            "expected a timeout, got {error:?}"
        );
        assert!(started.elapsed() < Duration::from_secs(30));
    }

    #[tokio::test]
    async fn a_timed_out_run_still_says_where_its_logs_are_and_closes_them() {
        let directory = TempDir::new().expect("a temporary directory");
        let root = directory.path().join("logs");
        let script = r#"
echo '{"type":"assistant","message":{"content":[{"type":"text","text":"Partial."}]}}'
echo 'still thinking' >&2
sleep 120
"#;
        let settings = settings(&directory, script)
            .with_timeout(Duration::from_secs(10))
            .with_log(&root);
        let provider = CliProvider::agent(AgentKind::Claude, settings);

        let failure = provider
            .execute(request(&[Message::user("hi")]), "test-run")
            .await
            .expect_err("a timeout");

        assert!(
            matches!(*failure.error, ProviderError::Timeout { .. }),
            "{failure:?}"
        );
        let files = failure.log.expect("the run's logs");
        assert_eq!(
            std::fs::read_to_string(&files.stdout).expect("the prose log"),
            "Partial."
        );
        let journal = std::fs::read_to_string(&files.events).expect("the journal");
        for record in [
            "subprocess_timed_out",
            "stdout_stream_closed",
            "stderr_stream_closed",
        ] {
            assert!(journal.contains(record), "{record} missing: {journal}");
        }
    }

    #[tokio::test]
    async fn a_timed_out_agent_that_ignores_termination_is_killed_with_its_group() {
        let directory = TempDir::new().expect("a temporary directory");
        let marker = directory.path().join("straggler");
        let script = format!(
            r#"trap '' TERM
sh -c 'trap "" TERM; sleep 60' &
echo $! > '{}'
sleep 60"#,
            marker.display()
        );
        let settings = settings(&directory, &script).with_timeout(Duration::from_secs(10));
        let provider = CliProvider::agent(AgentKind::Claude, settings);

        let error = run(&provider, &[Message::user("hi")])
            .await
            .expect_err("a timeout");

        assert!(matches!(error, ProviderError::Timeout { .. }), "{error:?}");
        let straggler = std::fs::read_to_string(&marker)
            .expect("the straggler's pid")
            .trim()
            .to_string();
        let started = Instant::now();
        while alive(&straggler) && started.elapsed() < Duration::from_secs(5) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(!alive(&straggler), "{straggler} outlived the timeout");
    }

    #[tokio::test]
    async fn a_timeout_too_large_for_a_deadline_means_no_timeout() {
        let directory = TempDir::new().expect("a temporary directory");
        let settings = settings(&directory, CLAUDE_SESSION).with_timeout(Duration::MAX);
        let provider = CliProvider::agent(AgentKind::Claude, settings);

        let completion = run(&provider, &[Message::user("hi")])
            .await
            .expect("an answer");

        assert_eq!(
            completion.message.content.as_deref(),
            Some("Looking now. It is empty.")
        );
    }

    #[tokio::test]
    async fn a_missing_agent_names_the_command_that_was_missing() {
        let provider = CliProvider::agent(
            AgentKind::Codex,
            CliSettings::default().with_executable("/nonexistent/abnegate/codex"),
        );

        let error = run(&provider, &[Message::user("hi")])
            .await
            .expect_err("a failure");

        let ProviderError::Unavailable { executable, .. } = &error else {
            panic!("expected an unavailable agent, got {error:?}");
        };
        assert_eq!(executable, "/nonexistent/abnegate/codex");
    }

    #[tokio::test]
    async fn a_missing_working_directory_fails_to_start() {
        let directory = TempDir::new().expect("a temporary directory");
        let settings = settings(&directory, CLAUDE_SESSION)
            .with_working_directory("/nonexistent-directory-for-abnegate-agent-cli");
        let provider = CliProvider::agent(AgentKind::Claude, settings);

        let error = run(&provider, &[Message::user("hi")])
            .await
            .expect_err("a failure");

        assert!(
            matches!(error, ProviderError::Unavailable { .. }),
            "expected an unavailable agent, got {error:?}"
        );
    }

    #[tokio::test]
    async fn a_credential_the_agent_echoes_back_never_reaches_the_error() {
        let directory = TempDir::new().expect("a temporary directory");
        let script = "echo \"fatal: rejected credential $ANTHROPIC_API_KEY\" >&2\nexit 1";
        let settings = settings(&directory, script).with_credential(Credential::key(
            "ANTHROPIC_API_KEY",
            concat!("sk-ant-", "api03-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"),
        ));
        let provider = CliProvider::agent(AgentKind::Claude, settings);

        let error = run(&provider, &[Message::user("hi")])
            .await
            .expect_err("a failure");

        let rendered = format!("{error} {error:?} {provider:?}");
        assert!(
            !rendered.contains(concat!("sk-ant-", "api03-AAAA")),
            "credential leaked: {rendered}"
        );
        assert!(rendered.contains("[REDACTED]"), "not redacted: {rendered}");
    }

    #[tokio::test]
    async fn a_credential_the_agent_echoes_back_never_reaches_the_execution() {
        let directory = TempDir::new().expect("a temporary directory");
        let script = "echo \"fatal: rejected credential $ANTHROPIC_API_KEY\" >&2\nexit 1";
        let settings = settings(&directory, script).with_credential(Credential::key(
            "ANTHROPIC_API_KEY",
            concat!("sk-ant-", "api03-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"),
        ));
        let provider = CliProvider::agent(AgentKind::Claude, settings);

        let execution = execute(&provider, &[Message::user("hi")]).await;

        assert_eq!(execution.status, ExitStatus::Code(1));
        assert!(!execution.stderr.contains(concat!("sk-ant-", "api03-AAAA")));
        assert!(execution.stderr.contains("[REDACTED]"));
    }

    #[tokio::test]
    async fn the_credential_reaches_the_agent_that_needs_it() {
        let directory = TempDir::new().expect("a temporary directory");
        let script = r#"
printf '{"type":"assistant","message":{"content":[{"type":"text","text":"%s"}]}}\n' "${ANTHROPIC_API_KEY:-absent}"
echo '{"type":"result","subtype":"success","is_error":false}'
"#;
        let settings = settings(&directory, script).with_credential(Credential::key(
            "ANTHROPIC_API_KEY",
            concat!("sk-ant-", "present"),
        ));
        let provider = CliProvider::agent(AgentKind::Claude, settings);

        let completion = run(&provider, &[Message::user("hi")])
            .await
            .expect("an answer");

        assert_eq!(
            completion.message.content.as_deref(),
            Some(concat!("sk-ant-", "present"))
        );
    }

    #[tokio::test]
    async fn an_explicit_environment_reaches_the_agent_and_the_credential_wins() {
        let directory = TempDir::new().expect("a temporary directory");
        let script = r#"
printf '{"type":"assistant","message":{"content":[{"type":"text","text":"%s|%s"}]}}\n' "${LINEAR_ISSUE_ID:-absent}" "${ANTHROPIC_API_KEY:-absent}"
echo '{"type":"result","subtype":"success","is_error":false}'
"#;
        let settings = settings(&directory, script)
            .with_environment("LINEAR_ISSUE_ID", "ENG-42")
            .with_environment("ANTHROPIC_API_KEY", "from-the-environment")
            .with_credential(Credential::key("ANTHROPIC_API_KEY", "from-the-credential"));
        let provider = CliProvider::agent(AgentKind::Claude, settings);

        let completion = run(&provider, &[Message::user("hi")])
            .await
            .expect("an answer");

        assert_eq!(
            completion.message.content.as_deref(),
            Some("ENG-42|from-the-credential")
        );
    }

    fn variables(path: &Path) -> Vec<String> {
        std::fs::read_to_string(path)
            .expect("the child's environment")
            .lines()
            .filter_map(|line| line.split_once('=').map(|(name, _)| name.to_string()))
            .collect()
    }

    fn recording_environment(recorded: &Path) -> String {
        format!(
            r#"env > '{}'
echo '{{"type":"result","subtype":"success","is_error":false}}'"#,
            recorded.display()
        )
    }

    #[tokio::test]
    async fn a_child_is_given_only_the_allowlist_and_what_it_was_handed() {
        let directory = TempDir::new().expect("a temporary directory");
        let recorded = directory.path().join("environment");
        let settings = settings(&directory, &recording_environment(&recorded))
            .with_environment("LINEAR_ISSUE_ID", "ENG-42")
            .with_credential(Credential::key(
                "ANTHROPIC_API_KEY",
                concat!("sk-ant-", "explicit"),
            ));
        let provider = CliProvider::agent(AgentKind::Claude, settings);

        run(&provider, &[Message::user("hi")])
            .await
            .expect("an answer");

        let names = variables(&recorded);
        let shell = ["PWD", "SHLVL", "_", "OLDPWD"];
        for name in &names {
            assert!(
                DEFAULT_ENVIRONMENT.contains(&name.as_str())
                    || AgentKind::Claude.configuration().contains(&name.as_str())
                    || shell.contains(&name.as_str())
                    || ["LINEAR_ISSUE_ID", "ANTHROPIC_API_KEY"].contains(&name.as_str()),
                "the child was handed {name} from the host: {names:?}"
            );
        }
        assert!(names.contains(&"LINEAR_ISSUE_ID".to_string()));
        assert!(names.contains(&"PATH".to_string()));
    }

    #[tokio::test]
    async fn an_opted_in_child_inherits_the_hosts_environment() {
        let Ok(package) = std::env::var("CARGO_PKG_NAME") else {
            return;
        };
        let directory = TempDir::new().expect("a temporary directory");
        let recorded = directory.path().join("environment");
        let script = recording_environment(&recorded);

        let confined = CliProvider::agent(AgentKind::Claude, settings(&directory, &script));
        run(&confined, &[Message::user("hi")])
            .await
            .expect("an answer");
        assert!(!variables(&recorded).contains(&"CARGO_PKG_NAME".to_string()));

        let inheriting = CliProvider::agent(
            AgentKind::Claude,
            settings(&directory, &script).inherit_environment(),
        );
        run(&inheriting, &[Message::user("hi")])
            .await
            .expect("an answer");
        let contents = std::fs::read_to_string(&recorded).expect("the child's environment");
        assert!(
            contents.contains(&format!("CARGO_PKG_NAME={package}")),
            "{contents}"
        );
        assert!(!variables(&recorded).contains(&"CLAUDECODE".to_string()));
    }

    #[tokio::test]
    async fn an_mcp_literal_reaches_the_child_through_its_environment_not_the_file() {
        let directory = TempDir::new().expect("a temporary directory");
        let copied = directory.path().join("mcp.json");
        let recorded = directory.path().join("environment");
        let script = format!(
            r#"env > '{recorded}'
while [ $# -gt 0 ]; do
  if [ "$1" = "--mcp-config" ]; then cp "$2" '{copied}'; fi
  shift
done
echo '{{"type":"result","subtype":"success","is_error":false}}'"#,
            recorded = recorded.display(),
            copied = copied.display(),
        );
        let settings = settings(&directory, &script).with_mcp_server(
            "grafana",
            McpServer {
                command: Some("uvx".to_string()),
                environment: [
                    (
                        "GRAFANA_TOKEN".to_string(),
                        SecretValue::new("glsa_realsecret"),
                    ),
                    (
                        "GRAFANA_PACKAGE".to_string(),
                        SecretValue::new("${CARGO_PKG_NAME}"),
                    ),
                ]
                .into(),
                ..McpServer::default()
            },
        );
        let provider = CliProvider::agent(AgentKind::Claude, settings);

        run(&provider, &[Message::user("hi")])
            .await
            .expect("an answer");

        let file = std::fs::read_to_string(&copied).expect("the MCP config");
        assert!(!file.contains("glsa_realsecret"), "{file}");
        let document: Value = serde_json::from_str(&file).expect("JSON");
        assert_eq!(
            document["mcpServers"]["grafana"]["env"]["GRAFANA_TOKEN"],
            "${ABNEGATE_MCP_0}"
        );
        let environment = std::fs::read_to_string(&recorded).expect("the child's environment");
        assert!(environment.contains("ABNEGATE_MCP_0=glsa_realsecret"));
        if let Ok(package) = std::env::var("CARGO_PKG_NAME") {
            assert!(environment.contains(&format!("CARGO_PKG_NAME={package}")));
        }
    }

    #[tokio::test]
    async fn a_codex_session_is_driven_by_the_same_provider() {
        let directory = TempDir::new().expect("a temporary directory");
        let script = r#"
echo '{"type":"thread.started","thread_id":"t1"}'
echo '{"type":"item.completed","item":{"id":"item_2","type":"agent_message","text":"The suite passes."}}'
echo '{"type":"turn.completed","usage":{"input_tokens":40,"output_tokens":8}}'
"#;
        let provider = CliProvider::agent(AgentKind::Codex, settings(&directory, script));

        let completion = run(&provider, &[Message::user("run the tests")])
            .await
            .expect("an answer");

        assert_eq!(completion.provider, "codex");
        assert_eq!(
            completion.message.content.as_deref(),
            Some("The suite passes.")
        );
        assert_eq!(completion.usage.expect("token counts").total_tokens, 48);
    }

    #[tokio::test]
    async fn a_codex_reconnect_notice_does_not_cut_a_completing_turn_short() {
        let directory = TempDir::new().expect("a temporary directory");
        let script = r#"
echo '{"type":"thread.started","thread_id":"t1"}'
echo '{"type":"turn.started"}'
echo '{"type":"error","message":"Reconnecting... 1/5 (stream disconnected before completion)"}'
sleep 1
echo '{"type":"item.completed","item":{"id":"item_0","type":"agent_message","text":"The suite passes."}}'
echo '{"type":"turn.completed","usage":{"input_tokens":40,"output_tokens":8}}'
"#;
        let provider = CliProvider::agent(AgentKind::Codex, settings(&directory, script));

        let completion = run(&provider, &[Message::user("run the tests")])
            .await
            .expect("an answer");

        assert_eq!(
            completion.message.content.as_deref(),
            Some("The suite passes.")
        );
    }

    #[tokio::test]
    async fn a_codex_error_with_no_turn_after_it_is_the_runs_failure() {
        let directory = TempDir::new().expect("a temporary directory");
        let script = r#"
echo '{"type":"turn.started"}'
echo '{"type":"error","message":"You have hit your usage limit. Try again later."}'
"#;
        let provider = CliProvider::agent(AgentKind::Codex, settings(&directory, script));

        let error = run(&provider, &[Message::user("hi")])
            .await
            .expect_err("a failure");

        let ProviderError::Agent { message, .. } = &error else {
            panic!("expected the agent's own failure, got {error:?}");
        };
        assert!(message.contains("usage limit"), "{message}");
    }

    #[tokio::test]
    async fn codex_refuses_claude_only_settings_before_starting_anything() {
        let provider = CliProvider::agent(
            AgentKind::Codex,
            CliSettings::default()
                .with_executable("/nonexistent/abnegate/codex")
                .read_only(),
        );

        let error = run(&provider, &[Message::user("hi")])
            .await
            .expect_err("a failure");

        assert!(
            matches!(error, ProviderError::Unsupported { .. }),
            "expected a refusal, got {error:?}"
        );
    }

    fn oversized(event: &str, filler: &str) -> String {
        format!(
            r#"printf '%s' '{event}'
head -c 5000 /dev/zero | tr '\0' 'x'
printf '%s\n' '{filler}'"#
        )
    }

    #[tokio::test]
    async fn an_oversized_tool_result_is_dropped_and_the_run_goes_on() {
        let directory = TempDir::new().expect("a temporary directory");
        let root = directory.path().join("logs");
        let script = format!(
            r#"{}
echo '{{"type":"assistant","message":{{"content":[{{"type":"text","text":"Read the image."}}]}}}}'
echo '{{"type":"result","subtype":"success","is_error":false}}'"#,
            oversized(
                r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t","content":[{"type":"image","source":{"type":"base64","data":""#,
                r#""}}]}]}}"#,
            )
        );
        let settings = settings(&directory, &script)
            .with_line_limit(1024)
            .with_log(&root);
        let provider = CliProvider::agent(AgentKind::Claude, settings);

        let execution = execute(&provider, &[Message::user("hi")]).await;

        assert_eq!(execution.stdout.dropped, 1);
        assert_eq!(execution.stdout.text, "Read the image.");
        let journal = std::fs::read_to_string(&execution.log.clone().expect("logs").events)
            .expect("the journal");
        assert!(journal.contains("stdout_line_dropped"), "{journal}");
        let completion = provider.assemble(execution).expect("an answer");
        assert_eq!(
            completion.message.content.as_deref(),
            Some("Read the image.")
        );
    }

    #[tokio::test]
    async fn an_oversized_tool_call_is_dropped_and_the_run_goes_on() {
        let directory = TempDir::new().expect("a temporary directory");
        let script = format!(
            r#"{}
echo '{{"type":"assistant","message":{{"content":[{{"type":"text","text":"Wrote the file."}}]}}}}'
echo '{{"type":"result","subtype":"success","is_error":false}}'"#,
            oversized(
                r#"{"type":"assistant","message":{"id":"msg_1","type":"message","content":[{"type":"tool_use","id":"toolu_01","name":"Write","input":{"file_path":"/w/big.rs","content":""#,
                r#""}}]}}"#,
            )
        );
        let settings = settings(&directory, &script).with_line_limit(1024);
        let provider = CliProvider::agent(AgentKind::Claude, settings);

        let execution = execute(&provider, &[Message::user("hi")]).await;

        assert_eq!(execution.stdout.dropped, 1);
        assert!(execution.stdout.finished);
        let completion = provider.assemble(execution).expect("an answer");
        assert_eq!(
            completion.message.content.as_deref(),
            Some("Wrote the file.")
        );
    }

    #[tokio::test]
    async fn an_oversized_result_or_reply_is_reported_as_malformed_output() {
        let directory = TempDir::new().expect("a temporary directory");
        for (event, filler) in [
            (
                r#"{"type":"result","subtype":"success","is_error":false,"result":""#,
                r#""}"#,
            ),
            (
                r#"{"type":"assistant","message":{"content":[{"type":"text","text":""#,
                r#""}]}}"#,
            ),
        ] {
            let settings = settings(&directory, &oversized(event, filler)).with_line_limit(256);
            let provider = CliProvider::agent(AgentKind::Claude, settings);

            let error = run(&provider, &[Message::user("hi")])
                .await
                .expect_err("a failure");

            assert!(
                matches!(&error, ProviderError::Malformed { message, .. } if message.contains("256 bytes")),
                "expected malformed output for {event}, got {error:?}"
            );
        }
    }

    #[tokio::test]
    async fn an_oversized_codex_command_output_is_dropped_but_its_prose_is_not() {
        let directory = TempDir::new().expect("a temporary directory");
        let script = format!(
            r#"{}
echo '{{"type":"item.completed","item":{{"id":"item_2","type":"agent_message","text":"Logged."}}}}'
echo '{{"type":"turn.completed"}}'"#,
            oversized(
                r#"{"type":"item.completed","item":{"id":"item_1","type":"command_execution","command":"cat big.log","aggregated_output":""#,
                r#""}}"#,
            )
        );
        let provider = CliProvider::agent(
            AgentKind::Codex,
            settings(&directory, &script).with_line_limit(1024),
        );
        let completion = run(&provider, &[Message::user("hi")])
            .await
            .expect("an answer");
        assert_eq!(completion.message.content.as_deref(), Some("Logged."));

        let prose = oversized(
            r#"{"type":"item.completed","item":{"id":"item_2","type":"agent_message","text":""#,
            r#""}}"#,
        );
        let provider = CliProvider::agent(
            AgentKind::Codex,
            settings(&directory, &prose).with_line_limit(1024),
        );
        let error = run(&provider, &[Message::user("hi")])
            .await
            .expect_err("a failure");
        assert!(
            matches!(error, ProviderError::Malformed { .. }),
            "{error:?}"
        );
    }

    #[tokio::test]
    async fn prose_past_the_limit_abandons_the_run() {
        let directory = TempDir::new().expect("a temporary directory");
        let script = r#"
for i in $(seq 1 100); do
  echo '{"type":"assistant","message":{"content":[{"type":"text","text":"0123456789012345678901234567890123456789"}]}}'
done
echo '{"type":"result","subtype":"success","is_error":false}'
"#;
        let settings = settings(&directory, script).with_output_limit(1024);
        let provider = CliProvider::agent(AgentKind::Claude, settings);

        let error = run(&provider, &[Message::user("hi")])
            .await
            .expect_err("a failure");

        assert!(
            matches!(&error, ProviderError::Malformed { message, .. } if message.contains("prose exceeded 1024 bytes")),
            "expected an overflow, got {error:?}"
        );
    }

    /// Prose that fills the limit exactly is all the run may keep, and an
    /// empty piece after it adds nothing, so the run still completes.
    #[tokio::test]
    async fn empty_prose_after_a_full_limit_is_not_an_overflow() {
        const PROSE: &str = "0123456789012345678901234567890123456789";
        let directory = TempDir::new().expect("a temporary directory");
        let script = format!(
            r#"
echo '{{"type":"assistant","message":{{"content":[{{"type":"text","text":"{PROSE}"}}]}}}}'
echo '{{"type":"assistant","message":{{"content":[{{"type":"text","text":""}}]}}}}'
echo '{{"type":"result","subtype":"success","is_error":false}}'
"#
        );
        let settings = settings(&directory, &script).with_output_limit(PROSE.len());
        let provider = CliProvider::agent(AgentKind::Claude, settings);

        let completion = run(&provider, &[Message::user("hi")])
            .await
            .expect("an answer");

        assert_eq!(completion.message.content.as_deref(), Some(PROSE));
    }

    #[tokio::test]
    async fn a_long_stream_around_short_prose_is_not_mistaken_for_runaway_output() {
        let directory = TempDir::new().expect("a temporary directory");
        let script = r#"
yes '{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t","content":"0123456789012345678901234567890123456789"}]}}' | head -n 2000
echo '{"type":"assistant","message":{"content":[{"type":"text","text":"All read."}]}}'
echo '{"type":"result","subtype":"success","is_error":false}'
"#;
        let settings = settings(&directory, script).with_output_limit(1024);
        let provider = CliProvider::agent(AgentKind::Claude, settings);

        let completion = run(&provider, &[Message::user("hi")])
            .await
            .expect("an answer");

        assert_eq!(completion.message.content.as_deref(), Some("All read."));
    }

    #[tokio::test]
    async fn a_finished_agent_that_will_not_exit_still_answers() {
        let directory = TempDir::new().expect("a temporary directory");
        let script = r#"
echo '{"type":"assistant","message":{"content":[{"type":"text","text":"Done."}]}}'
echo '{"type":"result","subtype":"success","is_error":false}'
sleep 120
"#;
        let provider = CliProvider::agent(AgentKind::Claude, settings(&directory, script));

        let started = Instant::now();
        let execution = execute(&provider, &[Message::user("hi")]).await;

        assert!(
            started.elapsed() < Duration::from_secs(60),
            "waited {:?} on a finished agent",
            started.elapsed()
        );
        assert_eq!(
            execution.stopped.as_deref(),
            Some("the agent finished its turn but did not exit")
        );
        assert_eq!(execution.stdout.text, "Done.");

        let completion = provider.assemble(execution).expect("the finished answer");
        assert_eq!(completion.message.content.as_deref(), Some("Done."));
    }

    #[tokio::test]
    async fn a_failed_run_takes_what_the_agent_forked_with_it() {
        let directory = TempDir::new().expect("a temporary directory");
        let marker = directory.path().join("straggler");
        let script = format!(
            r#"sleep 60 >/dev/null 2>&1 &
echo $! > '{}'
echo '{{"type":"rate_limit_event","rate_limit_info":{{"status":"rejected","rateLimitType":"five_hour"}}}}'"#,
            marker.display()
        );
        let provider = CliProvider::agent(AgentKind::Claude, settings(&directory, &script));

        let error = run(&provider, &[Message::user("hi")])
            .await
            .expect_err("a failure");
        assert!(error.to_string().contains("rate limit reached"), "{error}");

        let straggler = std::fs::read_to_string(&marker)
            .expect("the straggler's pid")
            .trim()
            .to_string();
        let started = Instant::now();
        while alive(&straggler) && started.elapsed() < Duration::from_secs(5) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(
            !alive(&straggler),
            "the failed run left {straggler} running"
        );
    }

    #[tokio::test]
    async fn a_run_that_trips_and_exits_cleanly_still_takes_what_it_forked_with_it() {
        let directory = TempDir::new().expect("a temporary directory");
        let marker = directory.path().join("straggler");
        let script = format!(
            r#"sleep 60 >/dev/null 2>&1 &
echo $! > '{}'
echo 'API Error: 429 Too Many Requests' >&2
sleep 1
exit 0"#,
            marker.display()
        );
        let settings = settings(&directory, &script).with_tripwire(|line| line.contains("429"));
        let provider = CliProvider::agent(AgentKind::Claude, settings);

        let error = run(&provider, &[Message::user("hi")])
            .await
            .expect_err("a failure");
        assert!(error.to_string().contains("429"), "{error}");

        let straggler = std::fs::read_to_string(&marker)
            .expect("the straggler's pid")
            .trim()
            .to_string();
        let started = Instant::now();
        while alive(&straggler) && started.elapsed() < Duration::from_secs(5) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(
            !alive(&straggler),
            "the failed run left {straggler} running"
        );
    }

    #[tokio::test]
    async fn a_successful_run_leaves_what_the_agent_forked_alone() {
        let directory = TempDir::new().expect("a temporary directory");
        let marker = directory.path().join("server");
        let script = format!(
            r#"sleep 60 >/dev/null 2>&1 &
echo $! > '{}'
echo '{{"type":"result","subtype":"success","is_error":false}}'"#,
            marker.display()
        );
        let provider = CliProvider::agent(AgentKind::Claude, settings(&directory, &script));

        run(&provider, &[Message::user("hi")])
            .await
            .expect("an answer");

        let server = std::fs::read_to_string(&marker)
            .expect("the server's pid")
            .trim()
            .to_string();
        assert!(alive(&server), "a successful run killed {server}");
        let _ = std::process::Command::new("kill").arg(&server).status();
    }

    #[tokio::test]
    async fn output_held_open_outside_the_group_keeps_what_was_read() {
        let directory = TempDir::new().expect("a temporary directory");
        let marker = directory.path().join("escaped");
        let script = format!(
            r#"perl -e 'use POSIX qw(setsid); setsid(); open(my $file, ">", $ARGV[0]) or die; print $file $$; close($file); sleep 60' '{}' &
echo '{{"type":"assistant","message":{{"content":[{{"type":"text","text":"Kept."}}]}}}}'
echo '{{"type":"result","subtype":"success","is_error":false}}'"#,
            marker.display()
        );
        let provider = CliProvider::agent(AgentKind::Claude, settings(&directory, &script));

        let started = Instant::now();
        let completion = run(&provider, &[Message::user("hi")]).await;

        let escaped = std::fs::read_to_string(&marker).unwrap_or_default();
        let _ = std::process::Command::new("kill")
            .arg(escaped.trim())
            .status();
        assert_eq!(
            completion.expect("an answer").message.content.as_deref(),
            Some("Kept.")
        );
        assert!(started.elapsed() < Duration::from_secs(60));
    }

    #[tokio::test]
    async fn a_tripwire_on_stderr_stops_an_agent_retrying_against_a_limit() {
        let directory = TempDir::new().expect("a temporary directory");
        let script = "echo 'API Error: 429 Too Many Requests, retrying in 60s' >&2
sleep 120";
        let settings = settings(&directory, script)
            .with_timeout(Duration::from_secs(90))
            .with_tripwire(|line| line.contains("429"));
        let provider = CliProvider::agent(AgentKind::Claude, settings);

        let started = Instant::now();
        let error = run(&provider, &[Message::user("hi")])
            .await
            .expect_err("a failure");

        assert!(started.elapsed() < Duration::from_secs(60));
        let ProviderError::Agent { message, .. } = &error else {
            panic!("expected the tripped line as the failure, got {error:?}");
        };
        assert!(message.contains("429 Too Many Requests"), "{message}");
    }

    #[tokio::test]
    async fn a_tripped_diagnostic_survives_an_agent_that_exits_by_itself() {
        let directory = TempDir::new().expect("a temporary directory");
        for script in [
            "echo 'API Error: 429 Too Many Requests' >&2\nsleep 1\nexit 0",
            "echo 'API Error: 429 Too Many Requests' >&2\nexit 0",
        ] {
            let settings = settings(&directory, script).with_tripwire(|line| line.contains("429"));
            let provider = CliProvider::agent(AgentKind::Claude, settings);

            let execution = execute(&provider, &[Message::user("hi")]).await;

            assert!(
                execution
                    .failure
                    .as_deref()
                    .is_some_and(|failure| failure.contains("429")),
                "{script}: {:?}",
                execution.failure
            );
            let error = provider.assemble(execution).expect_err("a failure");
            let ProviderError::Agent { message, .. } = &error else {
                panic!("expected the tripped line as the failure, got {error:?}");
            };
            assert!(
                message.contains("429 Too Many Requests"),
                "{script}: {message}"
            );
        }
    }

    #[tokio::test]
    async fn stdout_never_trips_the_tripwire() {
        let directory = TempDir::new().expect("a temporary directory");
        let script = r#"
echo '{"type":"assistant","message":{"content":[{"type":"text","text":"The handler returns 429."}]}}'
echo '{"type":"result","subtype":"success","is_error":false}'
"#;
        let settings = settings(&directory, script).with_tripwire(|line| line.contains("429"));
        let provider = CliProvider::agent(AgentKind::Claude, settings);

        let completion = run(&provider, &[Message::user("hi")])
            .await
            .expect("an answer");
        assert_eq!(
            completion.message.content.as_deref(),
            Some("The handler returns 429.")
        );
    }

    #[tokio::test]
    async fn a_secret_that_does_not_look_like_one_is_scrubbed_everywhere() {
        let directory = TempDir::new().expect("a temporary directory");
        let root = directory.path().join("logs");
        let script = r#"
echo "login failed for $DB_PASSWORD" >&2
printf '{"type":"result","subtype":"error_during_execution","is_error":true,"result":"rejected %s"}
' "$DB_PASSWORD"
"#;
        let settings = settings(&directory, script)
            .with_log(&root)
            .with_credential(Credential::key("DB_PASSWORD", "correct-horse-battery"));
        let provider = CliProvider::agent(AgentKind::Claude, settings);

        let execution = execute(&provider, &[Message::user("hi")]).await;

        assert_eq!(
            execution.stdout.failure.as_deref(),
            Some("rejected [REDACTED]")
        );
        assert!(!execution.stderr.contains("correct-horse-battery"));
        let files = execution.log.clone().expect("log files");
        for path in [&files.stdout, &files.stderr, &files.events] {
            let contents = std::fs::read_to_string(path).expect("a log file");
            assert!(
                !contents.contains("correct-horse-battery"),
                "{} leaked the secret",
                path.display()
            );
        }
        let error = provider.assemble(execution).expect_err("a failure");
        assert!(!error.to_string().contains("correct-horse-battery"));
    }

    #[tokio::test]
    async fn a_long_diagnostic_is_cut_down_in_the_error_but_kept_in_the_execution() {
        let directory = TempDir::new().expect("a temporary directory");
        let script = "head -c 10000 /dev/zero | tr '\\0' 'e' >&2
exit 2";
        let provider = CliProvider::agent(AgentKind::Claude, settings(&directory, script));

        let execution = execute(&provider, &[Message::user("hi")]).await;
        assert_eq!(execution.stderr.len(), 10_000);

        let error = provider.assemble(execution).expect_err("a failure");
        let ProviderError::Exit { message, .. } = &error else {
            panic!("expected an exit failure, got {error:?}");
        };
        assert!(message.len() <= 2000, "{} bytes", message.len());
        assert!(message.ends_with("..."));
    }

    #[tokio::test]
    async fn a_descendant_holding_the_output_open_is_reaped_after_the_agent_exits() {
        let directory = TempDir::new().expect("a temporary directory");
        let script = r#"
sleep 120 &
echo '{"type":"assistant","message":{"content":[{"type":"text","text":"done"}]}}'
echo '{"type":"result","subtype":"success","is_error":false}'
"#;
        let provider = CliProvider::agent(AgentKind::Claude, settings(&directory, script));

        let started = Instant::now();
        let completion = run(&provider, &[Message::user("hi")])
            .await
            .expect("an answer");

        assert_eq!(completion.message.content.as_deref(), Some("done"));
        assert!(
            started.elapsed() < Duration::from_secs(60),
            "waited {:?} on a straggler",
            started.elapsed()
        );
    }

    #[tokio::test]
    async fn a_cancelled_run_takes_its_process_tree_with_it() {
        let directory = TempDir::new().expect("a temporary directory");
        let marker = directory.path().join("group");
        let script = format!("echo $$ > '{}'\nsleep 120 &\nwait", marker.display());
        let provider = CliProvider::agent(AgentKind::Claude, settings(&directory, &script));

        let messages = [Message::user("hi")];
        let started = async {
            loop {
                let pid = std::fs::read_to_string(&marker)
                    .ok()
                    .and_then(|pid| pid.trim().parse::<u32>().ok());
                if let Some(pid) = pid {
                    return pid;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        };
        let leader = tokio::select! {
            result = run(&provider, &messages) => panic!("the run finished before it was cancelled: {result:?}"),
            () = tokio::time::sleep(Duration::from_secs(15)) => panic!("the agent never started"),
            pid = started => pid,
        };

        let started = Instant::now();
        while !running(leader).is_empty() && started.elapsed() < Duration::from_secs(5) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(
            running(leader).is_empty(),
            "the agent's group outlived the cancelled run: {:?}",
            running(leader)
        );
    }

    /// The members of process group `group` still running. A killed leader
    /// the runtime has yet to reap lingers as a zombie, which still counts as
    /// a member to a signal but runs nothing.
    fn running(group: u32) -> Vec<String> {
        let output = std::process::Command::new("ps")
            .args(["-A", "-o", "pid=,pgid=,stat="])
            .output()
            .expect("a process listing");
        let group = group.to_string();
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(|line| {
                let mut fields = line.split_whitespace();
                let (pid, pgid, state) = (fields.next()?, fields.next()?, fields.next()?);
                (pgid == group && !state.starts_with('Z')).then(|| pid.to_string())
            })
            .collect()
    }

    #[tokio::test]
    async fn an_mcp_config_is_attached_strictly_and_deleted_after_the_run() {
        let directory = TempDir::new().expect("a temporary directory");
        let captured = directory.path().join("arguments");
        let copied = directory.path().join("mcp.json");
        let script = format!(
            r#"printf '%s\n' "$@" > '{captured}'
while [ $# -gt 0 ]; do
  if [ "$1" = "--mcp-config" ]; then cp "$2" '{copied}'; fi
  shift
done
echo '{{"type":"result","subtype":"success","is_error":false}}'"#,
            captured = captured.display(),
            copied = copied.display(),
        );
        let settings = settings(&directory, &script)
            .with_permissions(["Read"])
            .with_mcp_server(
                "appwrite",
                McpServer {
                    command: Some("uvx".to_string()),
                    arguments: vec!["mcp-server-appwrite".to_string()],
                    environment: [(
                        "APPWRITE_API_KEY".to_string(),
                        SecretValue::new("${APPWRITE_API_KEY}"),
                    )]
                    .into(),
                    ..McpServer::default()
                },
            );
        let provider = CliProvider::agent(AgentKind::Claude, settings);

        run(&provider, &[Message::user("hi")])
            .await
            .expect("an answer");

        let arguments: Vec<String> = std::fs::read_to_string(&captured)
            .expect("the captured arguments")
            .lines()
            .map(str::to_string)
            .collect();
        let path = arguments
            .iter()
            .position(|argument| argument == "--mcp-config")
            .map(|index| PathBuf::from(&arguments[index + 1]))
            .expect("an --mcp-config flag");
        assert!(arguments.contains(&"--strict-mcp-config".to_string()));
        let tools: Vec<&str> = arguments
            .windows(2)
            .filter(|pair| pair[0] == "--allowedTools")
            .map(|pair| pair[1].as_str())
            .collect();
        assert_eq!(tools, ["Read", "mcp__appwrite"]);
        assert_eq!(arguments.last().map(String::as_str), Some("--print"));

        let document: Value =
            serde_json::from_str(&std::fs::read_to_string(&copied).expect("the MCP config"))
                .expect("JSON");
        assert_eq!(document["mcpServers"]["appwrite"]["command"], "uvx");
        assert_eq!(
            document["mcpServers"]["appwrite"]["env"]["APPWRITE_API_KEY"],
            "${APPWRITE_API_KEY}"
        );
        assert!(!path.exists(), "the MCP config outlived the run");
    }

    /// A stand-in for Claude that loads the repository's own settings, and
    /// runs the hook they declare, unless `--setting-sources` leaves the
    /// project out or `--restricted` leaves every settings file out, as the
    /// real CLI does.
    fn honouring_project_settings(captured: &Path) -> String {
        format!(
            r#"printf '%s\n' "$@" > '{captured}'
sources=user,project,local
previous=
restricted=
for argument in "$@"; do
  if [ "$previous" = "--setting-sources" ]; then sources="$argument"; fi
  if [ "$argument" = "--restricted" ]; then restricted=1; fi
  previous="$argument"
done
[ -n "$restricted" ] && sources=
case ",$sources," in
  *,project,*)
    hook=$(sed -n 's/.*"command": *"\([^"]*\)".*/\1/p' .claude/settings.json)
    [ -n "$hook" ] && sh -c "$hook"
    ;;
esac
echo '{{"type":"result","subtype":"success","is_error":false}}'"#,
            captured = captured.display(),
        )
    }

    #[tokio::test]
    async fn a_read_only_run_never_honours_the_repositorys_hooks() {
        let directory = TempDir::new().expect("a temporary directory");
        let repository = directory.path().join("repository");
        let marker = directory.path().join("hook-ran");
        std::fs::create_dir_all(repository.join(".claude")).expect("a settings directory");
        std::fs::write(
            repository.join(".claude/settings.json"),
            format!(
                r#"{{"hooks":{{"SessionStart":[{{"hooks":[{{"type":"command","command": "touch {}"}}]}}]}}}}"#,
                marker.display()
            ),
        )
        .expect("a hook-bearing settings file");
        let captured = directory.path().join("arguments");
        let settings = settings(&directory, &honouring_project_settings(&captured))
            .with_working_directory(&repository)
            .read_only();
        let provider = CliProvider::agent(AgentKind::Claude, settings);

        run(&provider, &[Message::user("hi")])
            .await
            .expect("an answer");

        assert!(!marker.exists(), "the repository's hook ran");
        let arguments: Vec<String> = std::fs::read_to_string(&captured)
            .expect("the captured arguments")
            .lines()
            .map(str::to_string)
            .collect();
        for expected in [
            ["--setting-sources", "user"],
            ["--tools", "Read,Grep,Glob"],
            ["--permission-mode", "dontAsk"],
            ["--permission-prompts", "none"],
            ["--allowedTools", "Read(./**)"],
        ] {
            assert!(
                arguments.windows(2).any(|pair| pair == expected),
                "{expected:?} missing from {arguments:?}"
            );
        }
        assert!(arguments.contains(&"--strict-mcp-config".to_string()));
        assert!(arguments.contains(&"--restricted".to_string()));
        assert!(
            !arguments.iter().any(|argument| argument.contains("Web")),
            "{arguments:?}"
        );
        assert!(!arguments.iter().any(|argument| argument == "--settings"));
    }

    #[tokio::test]
    async fn an_unconfined_run_leaves_the_setting_sources_to_the_agent() {
        let directory = TempDir::new().expect("a temporary directory");
        let repository = directory.path().join("repository");
        let marker = directory.path().join("hook-ran");
        std::fs::create_dir_all(repository.join(".claude")).expect("a settings directory");
        std::fs::write(
            repository.join(".claude/settings.json"),
            format!(r#"{{"command": "touch {}"}}"#, marker.display()),
        )
        .expect("a hook-bearing settings file");
        let captured = directory.path().join("arguments");
        let settings = settings(&directory, &honouring_project_settings(&captured))
            .with_working_directory(&repository);
        let provider = CliProvider::agent(AgentKind::Claude, settings);

        run(&provider, &[Message::user("hi")])
            .await
            .expect("an answer");

        assert!(
            marker.exists(),
            "the stand-in never loaded project settings"
        );
    }

    #[tokio::test]
    async fn a_read_only_run_refuses_a_bypass_before_starting_anything() {
        let directory = TempDir::new().expect("a temporary directory");
        let marker = directory.path().join("started");
        let settings = settings(&directory, &format!("touch '{}'", marker.display()))
            .read_only()
            .with_arguments(["--settings", r#"{"permissions":{"allow":["Bash"]}}"#]);
        let provider = CliProvider::agent(AgentKind::Claude, settings);

        let error = run(&provider, &[Message::user("hi")])
            .await
            .expect_err("a refusal");

        assert!(matches!(error, ProviderError::Config { .. }), "{error:?}");
        assert!(!marker.exists(), "the agent was started");
    }

    #[tokio::test]
    async fn instructions_far_larger_than_argv_allows_reach_the_agent_in_a_private_file() {
        let directory = TempDir::new().expect("a temporary directory");
        let captured = directory.path().join("arguments");
        let recorded = directory.path().join("instructions");
        let script = format!(
            r#"printf '%s\n' "$@" > '{captured}'
while [ $# -gt 0 ]; do
  if [ "$1" = "--append-system-prompt-file" ]; then
    ls -l "$2" | cut -c1-10 > '{recorded}.mode'
    wc -c < "$2" | tr -d ' ' > '{recorded}'
  fi
  shift
done
echo '{{"type":"result","subtype":"success","is_error":false}}'"#,
            captured = captured.display(),
            recorded = recorded.display(),
        );
        let instructions = "Be terse. ".repeat(200 * 1024);
        let settings = settings(&directory, &script).with_instructions(instructions.clone());
        let provider = CliProvider::agent(AgentKind::Claude, settings);

        run(&provider, &[Message::user("hi")])
            .await
            .expect("an answer");

        let arguments = std::fs::read_to_string(&captured).expect("the captured arguments");
        assert!(!arguments.contains("Be terse."));
        assert!(arguments.contains("--append-system-prompt-file"));
        assert_eq!(
            std::fs::read_to_string(&recorded)
                .expect("the instructions' size")
                .trim(),
            instructions.len().to_string()
        );
        let mode_path = directory.path().join("instructions.mode");
        assert_eq!(
            std::fs::read_to_string(&mode_path)
                .expect("the instructions' mode")
                .trim(),
            "-rw-------"
        );
    }

    #[tokio::test]
    async fn a_logged_run_keeps_its_prose_stderr_and_journal() {
        let directory = TempDir::new().expect("a temporary directory");
        let root = directory.path().join("logs");
        let provider = CliProvider::agent(
            AgentKind::Claude,
            settings(&directory, CLAUDE_SESSION).with_log(&root),
        );

        let execution = execute(&provider, &[Message::user("What does a.rs do?")]).await;

        let files = execution.log.expect("log files");
        assert!(files.stdout.starts_with(root.join("claude")));
        assert!(
            files
                .stdout
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with("_test-run.stdout.log"))
        );
        assert_eq!(
            std::fs::read_to_string(&files.stdout).expect("the prose log"),
            "Looking now. It is empty."
        );
        assert_eq!(
            std::fs::read_to_string(&files.stderr).expect("the stderr log"),
            "warming up the model\n"
        );

        let journal: Vec<Value> = std::fs::read_to_string(&files.events)
            .expect("the journal")
            .lines()
            .map(|line| serde_json::from_str(line).expect("a JSON line"))
            .collect();
        let events: Vec<&str> = journal
            .iter()
            .filter_map(|entry| entry["event"].as_str())
            .collect();
        for expected in [
            "execution_initialized",
            "subprocess_spawned",
            "stdout_line",
            "stderr_line",
            "stdout_stream_closed",
            "stderr_stream_closed",
            "subprocess_exited",
            "process_completed",
        ] {
            assert!(
                events.contains(&expected),
                "{expected} missing from {events:?}"
            );
        }
        assert_eq!(events.first(), Some(&"execution_initialized"));
        assert_eq!(events.last(), Some(&"process_completed"));
        assert!(journal.iter().all(|entry| entry["label"] == "test-run"));
        assert_eq!(
            journal
                .iter()
                .filter(|entry| entry["event"] == "stdout_line")
                .count(),
            5
        );
        let completed = journal.last().expect("a completion");
        assert_eq!(completed["data"]["has_structured_result"], true);
        assert_eq!(completed["data"]["finished"], true);
    }

    #[tokio::test]
    async fn a_journal_never_records_an_echoed_credential() {
        let directory = TempDir::new().expect("a temporary directory");
        let root = directory.path().join("logs");
        let script = r#"
printf '{"type":"assistant","message":{"content":[{"type":"text","text":"key %s"}]}}\n' "$ANTHROPIC_API_KEY"
echo "rejected $ANTHROPIC_API_KEY" >&2
echo '{"type":"result","subtype":"success","is_error":false}'
"#;
        let settings = settings(&directory, script)
            .with_log(&root)
            .with_credential(Credential::key(
                "ANTHROPIC_API_KEY",
                concat!("sk-ant-", "api03-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"),
            ));
        let provider = CliProvider::agent(AgentKind::Claude, settings);

        let execution = execute(&provider, &[Message::user("hi")]).await;

        let files = execution.log.expect("log files");
        for path in [&files.stdout, &files.stderr, &files.events] {
            let contents = std::fs::read_to_string(path).expect("a log file");
            assert!(
                !contents.contains(concat!("sk-ant-", "api03-AAAA")),
                "{} leaked the credential",
                path.display()
            );
        }
    }

    #[tokio::test]
    async fn a_secret_the_stream_escapes_or_a_short_one_never_reaches_a_log_or_an_error() {
        let directory = TempDir::new().expect("a temporary directory");
        let root = directory.path().join("logs");
        let script = r#"
printf '%s\n' '{"type":"assistant","message":{"content":[{"type":"text","text":"key pa\"ss\\word-123"}]}}'
echo "rejected pin $SERVICE_PIN" >&2
printf '%s\n' '{"type":"result","subtype":"error_during_execution","is_error":true,"result":"bad pin zq7x for pa\"ss\\word-123"}'
"#;
        let settings = settings(&directory, script)
            .with_log(&root)
            .with_environment("SERVICE_PIN", "zq7x")
            .with_credential(Credential::key("DB_PASSWORD", "pa\"ss\\word-123"));
        let provider = CliProvider::agent(AgentKind::Claude, settings);

        let execution = execute(&provider, &[Message::user("hi")]).await;

        let files = execution.log.clone().expect("log files");
        for path in [&files.stdout, &files.stderr, &files.events] {
            let contents = std::fs::read_to_string(path).expect("a log file");
            assert!(
                !contents.contains("word-123"),
                "{} leaked the secret: {contents}",
                path.display()
            );
            assert!(
                !contents.contains("zq7x"),
                "{} leaked the short secret: {contents}",
                path.display()
            );
        }
        assert!(!execution.stderr.contains("zq7x"), "{}", execution.stderr);
        let error = provider.assemble(execution).expect_err("a failure");
        let rendered = error.to_string();
        assert!(!rendered.contains("word-123"), "{rendered}");
        assert!(!rendered.contains("zq7x"), "{rendered}");
    }

    #[tokio::test]
    async fn a_secret_streamed_in_pieces_never_reaches_the_journal() {
        let directory = TempDir::new().expect("a temporary directory");
        let root = directory.path().join("logs");
        let secret = "Xk9Qz7Vw2Lp4";
        let script = r#"
rest="$SERVICE_TOKEN"
while [ -n "$rest" ]; do
  piece=$(printf '%s' "$rest" | cut -c1-3)
  rest=$(printf '%s' "$rest" | cut -c4-)
  printf '{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"%s"}},"session_id":"6f1"}\n' "$piece"
done
printf '{"type":"assistant","message":{"content":[{"type":"text","text":"token %s"}]}}\n' "$SERVICE_TOKEN"
echo '{"type":"result","subtype":"success","is_error":false}'
"#;
        let settings = settings(&directory, script)
            .with_log(&root)
            .with_arguments(["--include-partial-messages"])
            .with_environment("SERVICE_TOKEN", secret);
        let provider = CliProvider::agent(AgentKind::Claude, settings);

        let execution = execute(&provider, &[Message::user("hi")]).await;

        let files = execution.log.clone().expect("log files");
        let journal: Vec<Value> = std::fs::read_to_string(&files.events)
            .expect("the journal")
            .lines()
            .map(|line| serde_json::from_str(line).expect("a JSON line"))
            .collect();
        let streamed: String = journal
            .iter()
            .filter_map(|entry| entry["data"]["line"].as_str())
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .filter_map(|line| line["event"]["delta"]["text"].as_str().map(str::to_string))
            .collect();
        assert!(
            !streamed.contains(secret),
            "the journal holds the secret in pieces: {streamed}"
        );
        let contents = std::fs::read_to_string(&files.events).expect("the journal");
        for piece in ["Xk9", "Qz7", "Vw2", "Lp4"] {
            assert!(!contents.contains(piece), "{piece} reached the journal");
        }
        let closed = journal
            .iter()
            .find(|entry| entry["event"] == "stdout_stream_closed")
            .expect("the stream's close");
        assert_eq!(closed["data"]["line_count"], 6);
        assert_eq!(closed["data"]["partial_line_count"], 4);
        assert_eq!(
            std::fs::read_to_string(&files.stdout).expect("the prose log"),
            "token [REDACTED]"
        );
    }

    #[tokio::test]
    async fn a_provider_is_a_cli_provider_with_its_agents_capabilities() {
        let claude = CliProvider::agent(AgentKind::Claude, CliSettings::default());
        let codex = CliProvider::new("reviewer", AgentKind::Codex, CliSettings::default());

        assert_eq!(claude.kind(), ProviderKind::Cli);
        assert_eq!(claude.capabilities(), AgentKind::Claude.capabilities());
        assert_eq!(codex.name(), "reviewer");
        assert_eq!(codex.capabilities(), AgentKind::Codex.capabilities());
        assert_eq!(codex.settings().timeout, CliSettings::default().timeout);
    }
}
