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

use crate::STDERR_HEADING;
use crate::attachments::Attachments;
use crate::diagnostics;
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
use crate::log::tail_preview;
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
            if let Some(file) = &mcp.file {
                attachments = attachments.with_mcp(file.path());
            }
            attachments = attachments.with_mcp_overrides(&mcp.overrides);
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
                    "timeout_seconds": self.settings.timeout.as_secs_f64(),
                    "working_directory": self.settings.working_directory,
                    "arguments": logged,
                }),
            )
            .await;
        tracing::info!(
            provider = %self.name,
            agent = %self.agent,
            label,
            timeout_seconds = self.settings.timeout.as_secs_f64(),
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
                        json!({ "timeout_seconds": self.settings.timeout.as_secs_f64() }),
                    )
                    .await;
                tracing::warn!(provider = %self.name, label, "agent timed out; stopping its process group");
                stop_agent(child, group).await;
                return Err(ProviderError::timeout(&self.name, self.settings.timeout));
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
            return Err(ProviderError::agent(&self.name, &report(message, &stderr)));
        }

        let unstopped = stopped.is_none();
        match (failure.or(stopped), stdout.finished) {
            (Some(reason), false) => return Err(ProviderError::agent(&self.name, &reason)),
            _ if unstopped && status != ExitStatus::Code(0) => {
                let message = if stderr.trim().is_empty() {
                    NO_DIAGNOSTICS.to_string()
                } else {
                    tail_preview(stderr.trim_end(), EXECUTION_LOG_PREVIEW_LIMIT)
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
    /// the run still loads strictly, with no server at all, and without its
    /// MCP tools can still answer, and fails loudly on its own if it truly
    /// needed them.
    fn attach(&self) -> Option<McpAttachment> {
        let mcp = &self.settings.mcp;
        if mcp.is_empty() {
            return None;
        }
        match mcp.render(self.agent) {
            Ok(Some(attachment)) => {
                tracing::info!(
                    provider = %self.name,
                    servers = mcp.attachable(self.agent).count(),
                    "attaching MCP servers"
                );
                tracing::debug!(
                    provider = %self.name,
                    path = ?attachment.file.as_ref().map(|file| file.path().display().to_string()),
                    config = %mcp.redacted(self.agent),
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

/// `words`, then [`STDERR_HEADING`] and the last whole lines of `stderr` that
/// fit in 1 KiB: some agents give the reason for a failure only there.
fn report(words: &str, stderr: &str) -> String {
    let tail = diagnostics::tail(stderr);
    if tail.is_empty() {
        return words.to_string();
    }
    format!("{words}{STDERR_HEADING}{tail}")
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
        let _ = group.kill_until_gone().await;
        return Some(status);
    }
    let _ = group.kill_until_gone().await;
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
        let _ = group.kill_until_gone().await;
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
    use std::fmt::Debug;
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use std::path::PathBuf;
    use std::pin::Pin;
    use std::pin::pin;
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
    use serde_json::json;
    use tempfile::TempDir;

    use super::CliProvider;
    use super::report;
    use crate::PROSE_EXCEEDED;
    use crate::STDERR_HEADING;
    use crate::execution::Execution;
    use crate::kind::AgentKind;
    use crate::mcp::McpServer;
    use crate::mcp::McpTransport;
    use crate::mcp::expand;
    use crate::parser::claude::CREDITS_REQUIRED;
    use crate::parser::claude::LONG_CONTEXT_CREDITS_REQUIRED;
    use crate::settings::CliSettings;
    use crate::structured_result::StructuredResult;
    use crate::test_support::PATIENCE;
    use crate::test_support::captured_logs;
    use crate::test_support::delegated;
    use crate::test_support::running;

    const ETXTBSY: i32 = 26;
    const PROBE: &str = "FAKE_AGENT_PROBE";

    /// Beyond the two minutes a lingering fake agent sleeps, so a run that
    /// waits on one instead of stopping it sees it exit by itself first.
    const TIMEOUT: Duration = Duration::from_secs(300);

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

    /// Runs the fake once to completion, before a test's deadlines start.
    ///
    /// Linux refuses to exec a file any process still holds open for writing.
    /// The descriptor here is closed, but a sibling test forking between its
    /// own open and exec inherits it for that window, so a freshly written
    /// script can hit ETXTBSY under a parallel run. macOS assesses a new
    /// executable on its first run, which can take seconds, and a run killed
    /// at once leaves that to the next run. Production never meets either: a
    /// provider execs an installed binary, not one it just wrote.
    fn wait_until_executable(path: &Path) {
        for _ in 0..50 {
            match std::process::Command::new(path)
                .env(PROBE, "1")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
            {
                Err(error) if error.raw_os_error() == Some(ETXTBSY) => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                _ => return,
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
            .with_timeout(TIMEOUT)
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

    /// Drive `run` until the fake agent creates `marker`, and return what the
    /// run returned if it ended first, having created it.
    async fn reach<T: Debug>(run: Pin<&mut impl Future<Output = T>>, marker: &Path) -> Option<T> {
        let created = async {
            while !marker.exists() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        };
        tokio::select! {
            ended = run => {
                assert!(
                    marker.exists(),
                    "the run ended before the agent reached {}: {ended:?}",
                    marker.display()
                );
                Some(ended)
            }
            () = created => None,
            () = tokio::time::sleep(PATIENCE) => panic!("the agent never reached {} in {PATIENCE:?}", marker.display()),
        }
    }

    /// Drive `run` until the fake agent creates `marker`, then move the clock
    /// past its deadline. The clock runs on at once, so the run stops its
    /// agent and drains its output in real time rather than skipping each
    /// grace period.
    async fn expired<T: Debug>(run: impl Future<Output = T>, marker: &Path) -> T {
        let mut run = pin!(run);
        if let Some(ended) = reach(run.as_mut(), marker).await {
            return ended;
        }
        tokio::time::pause();
        tokio::time::advance(TIMEOUT).await;
        tokio::time::resume();
        run.await
    }

    /// Drive `run` until the fake agent creates `marker`, then time the rest
    /// of it.
    async fn timed<T: Debug>(run: impl Future<Output = T>, marker: &Path) -> (T, Duration) {
        let mut run = pin!(run);
        let ended = reach(run.as_mut(), marker).await;
        let reached = Instant::now();
        let output = match ended {
            Some(output) => output,
            None => run.await,
        };
        (output, reached.elapsed())
    }

    #[tokio::test]
    async fn a_run_that_ends_just_after_its_agent_reaches_the_marker_still_reached_it() {
        let directory = TempDir::new().expect("a temporary directory");
        let marker = directory.path().join("reached");
        let run = pin!(async {
            std::fs::File::create(&marker).expect("the marker");
            "ended"
        });

        assert_eq!(reach(run, &marker).await, Some("ended"));
    }

    #[tokio::test]
    #[should_panic(expected = "the run ended before the agent reached")]
    async fn a_run_that_ends_before_its_agent_reaches_the_marker_fails() {
        let directory = TempDir::new().expect("a temporary directory");
        let marker = directory.path().join("reached");

        reach(pin!(async { "ended" }), &marker).await;
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
echo '{"type":"assistant","message":{"content":[{"type":"text","text":"You have hit your limit"}]},"parent_tool_use_id":null,"error":"rate_limit","is_api_error_message":true}'
sleep 120
"#;
        let provider = CliProvider::agent(AgentKind::Claude, settings(&directory, script));

        let execution = execute(&provider, &[Message::user("hi")]).await;

        assert!(
            execution.stopped.is_some(),
            "the throttled agent was waited on until it exited"
        );
        let error = provider.assemble(execution).expect_err("a failure");
        let ProviderError::Agent { message, .. } = &error else {
            panic!("expected the agent's own failure, got {error:?}");
        };
        assert!(message.contains("rate limit reached"), "{message}");
        assert!(message.contains("five_hour"), "{message}");
        assert!(message.contains(r#""resetsAt":1772096400"#), "{message}");
        assert!(message.ends_with(": You have hit your limit"), "{message}");
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
        let spoken = directory.path().join("spoken");
        let script = format!(
            r#"
echo '{{"type":"assistant","message":{{"content":[{{"type":"text","text":"Partial."}}]}}}}'
echo 'still thinking' >&2
touch '{}'
sleep 120
"#,
            spoken.display()
        );
        let settings = settings(&directory, &script).with_log(&root);
        let provider = CliProvider::agent(AgentKind::Claude, settings);
        let messages = [Message::user("hi")];

        let failure = expired(provider.execute(request(&messages), "test-run"), &spoken)
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

    /// The journal gives the timeout in seconds with its fraction, so a
    /// limit under a second is not recorded as no time at all.
    #[tokio::test]
    async fn a_timeout_under_a_second_is_journalled_with_its_fraction() {
        let directory = TempDir::new().expect("a temporary directory");
        let root = directory.path().join("logs");
        let settings = settings(&directory, "sleep 120")
            .with_timeout(Duration::from_millis(500))
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
        let journal = std::fs::read_to_string(&files.events).expect("the journal");
        let journalled: Vec<Option<f64>> = journal
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).expect("a journal entry"))
            .filter(|entry| {
                matches!(
                    entry["event"].as_str(),
                    Some("execution_initialized" | "subprocess_timed_out")
                )
            })
            .map(|entry| entry["data"]["timeout_seconds"].as_f64())
            .collect();
        assert_eq!(journalled, [Some(0.5), Some(0.5)], "{journal}");
    }

    #[tokio::test]
    async fn a_timed_out_agent_that_ignores_termination_is_killed_with_its_group() {
        let directory = TempDir::new().expect("a temporary directory");
        let marker = directory.path().join("straggler");
        let ready = directory.path().join("ready");
        let script = format!(
            r#"trap '' TERM
sh -c 'trap "" TERM; echo $$ > "{marker}"; touch "{ready}"; sleep 120' &
sleep 120"#,
            marker = marker.display(),
            ready = ready.display(),
        );
        let provider = CliProvider::agent(AgentKind::Claude, settings(&directory, &script));
        let messages = [Message::user("hi")];

        let error = expired(run(&provider, &messages), &ready)
            .await
            .expect_err("a timeout");

        assert!(matches!(error, ProviderError::Timeout { .. }), "{error:?}");
        let straggler = std::fs::read_to_string(&marker)
            .expect("the straggler's pid")
            .trim()
            .to_string();
        let started = Instant::now();
        while alive(&straggler) && started.elapsed() < PATIENCE {
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

    /// What a host's environment can hold that no agent signed in with a
    /// credential of its own may be handed. Every value says `notreal`, so a
    /// leak shows up under any name.
    const HOST_SECRETS: &[(&str, &str)] = &[
        ("DATABASE_URL", "postgres://app:notrealpassword@db/app"),
        ("SESSION_SECRET", "notreal-session-secret"),
        ("ENCRYPTION_KEY", "notreal-encryption-key"),
        ("ANTHROPIC_API_KEY", concat!("sk-ant-", "notreal-key")),
        ("ANTHROPIC_AUTH_TOKEN", "notreal-auth-token"),
        (
            "CLAUDE_CODE_OAUTH_TOKEN",
            concat!("sk-ant-", "oat-notreal-token"),
        ),
        ("OPENAI_API_KEY", concat!("sk-", "notreal-openai-key")),
        ("CLAUDECODE", "notreal-session"),
    ];

    /// The host's secrets are set in this test's own child process. The
    /// caller signs the agent in with a key of its own and allows, beside a
    /// harmless name, the other variables the agent signs in with and its
    /// nested-session marker, none of which may pass.
    #[tokio::test]
    async fn the_hosts_secrets_never_reach_the_agent() {
        const NAME: &str = "provider::tests::the_hosts_secrets_never_reach_the_agent";
        const KEY: &str = concat!("sk-ant-", "explicit-key");
        const CERTIFICATE: &str = "/etc/ssl/corporate.pem";
        let mut host = HOST_SECRETS.to_vec();
        host.push(("CORPORATE_CA", CERTIFICATE));
        if delegated(NAME, &host).await {
            return;
        }
        let directory = TempDir::new().expect("a temporary directory");
        let recorded = directory.path().join("environment");
        let settings = settings(&directory, &recording_environment(&recorded))
            .with_credential(Credential::key("ANTHROPIC_API_KEY", KEY))
            .allow([
                "CORPORATE_CA",
                "ANTHROPIC_AUTH_TOKEN",
                "CLAUDE_CODE_OAUTH_TOKEN",
                "CLAUDECODE",
            ]);
        let provider = CliProvider::agent(AgentKind::Claude, settings);

        run(&provider, &[Message::user("hi")])
            .await
            .expect("an answer");

        let child = recorded_environment(&recorded);
        let leaked: Vec<&str> = HOST_SECRETS
            .iter()
            .map(|(name, _)| *name)
            .filter(|name| *name != "ANTHROPIC_API_KEY" && child.contains_key(*name))
            .collect();
        assert!(leaked.is_empty(), "these reached the agent: {leaked:?}");
        let carrying: Vec<&String> = child
            .iter()
            .filter(|(_, value)| value.contains("notreal"))
            .map(|(name, _)| name)
            .collect();
        assert!(
            carrying.is_empty(),
            "these carried a secret's value to the agent: {carrying:?}"
        );
        assert_eq!(
            child.get("ANTHROPIC_API_KEY").map(String::as_str),
            Some(KEY)
        );
        assert_eq!(
            child.get("CORPORATE_CA").map(String::as_str),
            Some(CERTIFICATE),
            "the allowed name was not honoured"
        );
    }

    #[tokio::test]
    async fn the_agent_still_finds_its_home_and_its_commands() {
        let directory = TempDir::new().expect("a temporary directory");
        let recorded = directory.path().join("environment");
        let provider = CliProvider::agent(
            AgentKind::Claude,
            settings(&directory, &recording_environment(&recorded)),
        );

        run(&provider, &[Message::user("hi")])
            .await
            .expect("an answer");

        let child = recorded_environment(&recorded);
        for name in ["HOME", "PATH"] {
            assert_eq!(
                child.get(name),
                std::env::var(name).ok().as_ref(),
                "{name} did not reach the agent as it was"
            );
        }
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
        let generated = |variable: &str| {
            document["mcpServers"]["grafana"]["env"][variable]
                .as_str()
                .and_then(|value| value.strip_prefix("${ABNEGATE_MCP_"))
                .and_then(|value| value.strip_suffix('}'))
                .map(|name| format!("ABNEGATE_MCP_{name}"))
                .expect("a generated variable")
        };
        let environment = std::fs::read_to_string(&recorded).expect("the child's environment");
        assert!(
            environment.contains(&format!("{}=glsa_realsecret", generated("GRAFANA_TOKEN"))),
            "{environment}"
        );
        if let Ok(package) = std::env::var("CARGO_PKG_NAME") {
            assert!(
                environment.contains(&format!("{}={package}", generated("GRAFANA_PACKAGE"))),
                "{environment}"
            );
            assert!(!variables(&recorded).contains(&"CARGO_PKG_NAME".to_string()));
        }
    }

    /// The child's environment a fake agent recorded with `env`.
    fn recorded_environment(path: &Path) -> std::collections::BTreeMap<String, String> {
        std::fs::read_to_string(path)
            .expect("the child's environment")
            .lines()
            .filter_map(|line| line.split_once('='))
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect()
    }

    /// `value`, from a remote server's entry, as the CLI sends it: expanded
    /// against the `child`'s environment when the CLI reads the file, and a
    /// header value again when it connects.
    fn sent(value: &Value, child: &std::collections::BTreeMap<String, String>) -> String {
        let lookup = |name: &str| child.get(name).cloned();
        expand(&expand(value.as_str().expect("text"), &lookup), &lookup)
    }

    /// A remote server's reference resolves to the secret bound to it, which
    /// reaches the child only inside the generated variable the file refers
    /// to, never under its own name, and never reaches a log, echoed alone or
    /// inside the header.
    #[tokio::test]
    async fn a_secret_bound_to_a_remote_server_reaches_it_and_never_a_log() {
        const TOKEN: &str = "lin-api-marker-9f3e27";
        let directory = TempDir::new().expect("a temporary directory");
        let root = directory.path().join("logs");
        let copied = directory.path().join("mcp.json");
        let environment = directory.path().join("environment");
        let script = format!(
            r#"env > '{environment}'
while [ $# -gt 0 ]; do
  if [ "$1" = "--mcp-config" ]; then cp "$2" '{copied}'; fi
  shift
done
header=$(sed -n 's/^ABNEGATE_MCP_[^=]*=//p' '{environment}')
token=${{header#Bearer }}
printf '{{"type":"assistant","message":{{"content":[{{"type":"text","text":"sent %s"}}]}}}}\n' "$token"
echo "sent $header" >&2
echo '{{"type":"result","subtype":"success","is_error":false}}'"#,
            environment = environment.display(),
            copied = copied.display(),
        );
        let settings = settings(&directory, &script)
            .with_log(&root)
            .with_mcp_server(
                "linear",
                McpServer::remote("https://mcp.linear.app/mcp")
                    .with_header("Authorization", "Bearer ${LINEAR_TOKEN}")
                    .with_secret("LINEAR_TOKEN", TOKEN),
            );
        let provider = CliProvider::agent(AgentKind::Claude, settings);

        let execution = execute(&provider, &[Message::user("hi")]).await;

        let file = std::fs::read_to_string(&copied).expect("the MCP config");
        assert!(!file.contains(TOKEN), "{file}");
        let document: Value = serde_json::from_str(&file).expect("JSON");
        let child = recorded_environment(&environment);
        assert_eq!(
            sent(
                &document["mcpServers"]["linear"]["headers"]["Authorization"],
                &child
            ),
            format!("Bearer {TOKEN}")
        );
        assert!(!child.contains_key("LINEAR_TOKEN"), "{child:?}");
        let files = execution.log.clone().expect("log files");
        for path in [&files.stdout, &files.stderr, &files.events] {
            let contents = std::fs::read_to_string(path).expect("a log file");
            assert!(
                !contents.contains(TOKEN),
                "{} leaked the token: {contents}",
                path.display()
            );
        }
        assert_eq!(
            std::fs::read_to_string(&files.stdout).expect("the prose log"),
            "sent [REDACTED]"
        );
    }

    /// A secret bound to a stdio server is what its references read first,
    /// before what the agent is given and this process's environment, and it
    /// reaches the server only under a generated name. The host's own `T` is
    /// set in this test's own child process.
    #[tokio::test]
    async fn a_secret_bound_to_a_stdio_server_wins_over_the_hosts_variable() {
        const NAME: &str =
            "provider::tests::a_secret_bound_to_a_stdio_server_wins_over_the_hosts_variable";
        const HOST: &str = "t-host-marker-3a8f";
        const BOUND: &str = "t-bound-marker-71c2";
        if delegated(NAME, &[("T", HOST)]).await {
            return;
        }
        let directory = TempDir::new().expect("a temporary directory");
        let copied = directory.path().join("mcp.json");
        let environment = directory.path().join("environment");
        let script = format!(
            r#"env > '{environment}'
while [ $# -gt 0 ]; do
  if [ "$1" = "--mcp-config" ]; then cp "$2" '{copied}'; fi
  shift
done
echo '{{"type":"result","subtype":"success","is_error":false}}'"#,
            environment = environment.display(),
            copied = copied.display(),
        );
        let settings = settings(&directory, &script).with_mcp_server(
            "notes",
            McpServer::command("notes-server", ["mcp"])
                .with_environment("T", "${T}")
                .with_secret("T", BOUND),
        );
        let provider = CliProvider::agent(AgentKind::Claude, settings);

        run(&provider, &[Message::user("hi")])
            .await
            .expect("an answer");

        let file = std::fs::read_to_string(&copied).expect("the MCP config");
        assert!(!file.contains(BOUND), "{file}");
        let document: Value = serde_json::from_str(&file).expect("JSON");
        let variable = document["mcpServers"]["notes"]["env"]["T"]
            .as_str()
            .and_then(|value| value.strip_prefix("${"))
            .and_then(|value| value.strip_suffix('}'))
            .expect("a generated variable");
        let child = recorded_environment(&environment);
        assert_eq!(child.get(variable).map(String::as_str), Some(BOUND));
        assert!(!child.contains_key("T"), "{child:?}");
        assert!(
            !child.values().any(|value| value.contains(HOST)),
            "{child:?}"
        );
    }

    /// A token the agent's own tools need, allowed through from this
    /// process's environment, must never reach a remote server that names
    /// it: Claude Code reads its own and cloud credentials as empty toward a
    /// remote server, but not `GITHUB_TOKEN`. The token is set in this test's
    /// own child process, and the file the agent is given is read as the CLI
    /// reads it, against the agent's environment.
    #[tokio::test]
    async fn a_remote_server_is_never_sent_a_token_allowed_through_to_the_agent() {
        const NAME: &str =
            "provider::tests::a_remote_server_is_never_sent_a_token_allowed_through_to_the_agent";
        const TOKEN: &str = "ghp-host-marker-5c1d";
        if delegated(NAME, &[("GITHUB_TOKEN", TOKEN)]).await {
            return;
        }
        let directory = TempDir::new().expect("a temporary directory");
        let copied = directory.path().join("mcp.json");
        let environment = directory.path().join("environment");
        let script = format!(
            r#"env > '{environment}'
while [ $# -gt 0 ]; do
  if [ "$1" = "--mcp-config" ]; then cp "$2" '{copied}'; fi
  shift
done
echo '{{"type":"result","subtype":"success","is_error":false}}'"#,
            environment = environment.display(),
            copied = copied.display(),
        );
        let settings = settings(&directory, &script)
            .allow(["GITHUB_TOKEN"])
            .with_mcp_server(
                "analytics",
                McpServer::remote("https://analytics.example/mcp")
                    .with_header("X-Auth", "${GITHUB_TOKEN}"),
            )
            .with_mcp_server(
                "tracker",
                McpServer::remote("https://tracker.example/${GITHUB_TOKEN:-public}/mcp")
                    .with_header("X-Auth", "token ${GITHUB_TOKEN:-anonymous}"),
            );
        let provider = CliProvider::agent(AgentKind::Claude, settings);

        run(&provider, &[Message::user("hi")])
            .await
            .expect("an answer");

        let child = recorded_environment(&environment);
        assert_eq!(
            child.get("GITHUB_TOKEN").map(String::as_str),
            Some(TOKEN),
            "the agent's own tools are handed the token"
        );
        let file = std::fs::read_to_string(&copied).expect("the MCP config");
        assert!(!file.contains(TOKEN), "{file}");
        let document: Value = serde_json::from_str(&file).expect("JSON");
        let servers = document["mcpServers"].as_object().expect("servers");
        for (name, server) in servers {
            let headers = server["headers"].as_object().into_iter().flatten();
            for value in std::iter::once(&server["url"]).chain(headers.map(|(_, value)| value)) {
                let sent = sent(value, &child);
                assert!(!sent.contains(TOKEN), "{name} is sent the token: {sent}");
            }
        }
        assert_eq!(servers.keys().collect::<Vec<_>>(), ["tracker"]);
        assert_eq!(
            sent(&servers["tracker"]["url"], &child),
            "https://tracker.example/public/mcp"
        );
        assert_eq!(
            sent(&servers["tracker"]["headers"]["X-Auth"], &child),
            "token anonymous"
        );
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

        let execution = execute(&provider, &[Message::user("hi")]).await;

        assert_eq!(
            execution.stopped.as_deref(),
            Some("the agent finished its turn but did not exit"),
            "the finished agent was waited on until it exited"
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
            r#"sleep 120 >/dev/null 2>&1 &
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
        while alive(&straggler) && started.elapsed() < PATIENCE {
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
            r#"sleep 120 >/dev/null 2>&1 &
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
        while alive(&straggler) && started.elapsed() < PATIENCE {
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
        let started = directory.path().join("started");
        let script = format!(
            r#"touch '{started}'
perl -e 'use POSIX qw(setsid); setsid(); open(my $file, ">", $ARGV[0]) or die; print $file $$; close($file); sleep 120' '{marker}' &
echo '{{"type":"assistant","message":{{"content":[{{"type":"text","text":"Kept."}}]}}}}'
echo '{{"type":"result","subtype":"success","is_error":false}}'"#,
            started = started.display(),
            marker = marker.display(),
        );
        let provider = CliProvider::agent(AgentKind::Claude, settings(&directory, &script));
        let messages = [Message::user("hi")];

        let (completion, waited) = timed(run(&provider, &messages), &started).await;

        let escaped = std::fs::read_to_string(&marker).unwrap_or_default();
        let _ = std::process::Command::new("kill")
            .arg(escaped.trim())
            .status();
        assert_eq!(
            completion.expect("an answer").message.content.as_deref(),
            Some("Kept.")
        );
        assert!(
            waited < PATIENCE,
            "waited {waited:?} on output held open outside the group"
        );
    }

    #[tokio::test]
    async fn a_tripwire_on_stderr_stops_an_agent_retrying_against_a_limit() {
        let directory = TempDir::new().expect("a temporary directory");
        let script = "echo 'API Error: 429 Too Many Requests, retrying in 60s' >&2
sleep 120";
        let settings = settings(&directory, script).with_tripwire(|line| line.contains("429"));
        let provider = CliProvider::agent(AgentKind::Claude, settings);

        let execution = execute(&provider, &[Message::user("hi")]).await;

        assert!(
            execution.stopped.is_some(),
            "the agent retrying against a limit was waited on until it exited"
        );
        let error = provider.assemble(execution).expect_err("a failure");
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
printf 'the last word' >&2
exit 2";
        let provider = CliProvider::agent(AgentKind::Claude, settings(&directory, script));

        let execution = execute(&provider, &[Message::user("hi")]).await;
        assert_eq!(execution.stderr.len(), 10_013);

        let error = provider.assemble(execution).expect_err("a failure");
        let ProviderError::Exit { message, .. } = &error else {
            panic!("expected an exit failure, got {error:?}");
        };
        assert!(message.len() <= 2000, "{} bytes", message.len());
        assert!(message.starts_with("..."), "{message}");
        assert!(message.ends_with("the last word"), "{message}");
    }

    #[tokio::test]
    async fn a_failed_codex_turn_carries_the_renewal_failure_codex_wrote_only_to_stderr() {
        let directory = TempDir::new().expect("a temporary directory");
        let recording = directory.path().join("recording.jsonl");
        std::fs::write(
            &recording,
            include_str!("../tests/fixtures/codex/refresh-invalidated.jsonl"),
        )
        .expect("the recording");
        let diagnostics = directory.path().join("diagnostics.stderr");
        std::fs::write(
            &diagnostics,
            include_str!("../tests/fixtures/codex/refresh-invalidated-errors.stderr"),
        )
        .expect("the diagnostics");
        let script = format!(
            "cat '{}' >&2\ncat '{}'\nexit 1",
            diagnostics.display(),
            recording.display()
        );
        let provider = CliProvider::agent(AgentKind::Codex, settings(&directory, &script));

        let error = run(&provider, &[Message::user("Echo the nonce.")])
            .await
            .expect_err("a failed turn");

        let rendered = error.to_string();
        assert!(
            rendered.contains("workspace routing discovery unauthorized (401)"),
            "lost codex's own wording: {rendered}"
        );
        assert!(
            rendered.contains("Your access token could not be refreshed"),
            "lost the reason codex gave on stderr: {rendered}"
        );
    }

    #[tokio::test]
    async fn stderr_past_the_output_cap_is_still_drained() {
        let directory = TempDir::new().expect("a temporary directory");
        let script = r#"
head -c 262144 /dev/zero | tr '\0' 'e' >&2
echo 'still logging' >&2
echo '{"type":"assistant","message":{"content":[{"type":"text","text":"Done."}]}}'
echo '{"type":"result","subtype":"success","is_error":false}'
"#;
        let provider = CliProvider::agent(
            AgentKind::Claude,
            settings(&directory, script).with_output_limit(4 * 1024),
        );

        let completion = run(&provider, &[Message::user("hi")])
            .await
            .expect("an answer from an agent that wrote 256 KiB to stderr");

        assert_eq!(completion.message.content.as_deref(), Some("Done."));
    }

    #[tokio::test]
    async fn a_failure_past_the_output_cap_reports_the_end_of_stderr() {
        let directory = TempDir::new().expect("a temporary directory");
        let script = r#"
head -c 262144 /dev/zero | tr '\0' 'e' >&2
echo 'error: the real reason' >&2
exit 3
"#;
        let provider = CliProvider::agent(
            AgentKind::Claude,
            settings(&directory, script).with_output_limit(4 * 1024),
        );

        let execution = execute(&provider, &[Message::user("hi")]).await;
        assert!(
            execution.stderr.len() <= 4 * 1024,
            "{} bytes",
            execution.stderr.len()
        );
        let error = provider.assemble(execution).expect_err("a failure");

        let ProviderError::Exit {
            status, message, ..
        } = &error
        else {
            panic!("expected an exit failure, got {error:?}");
        };
        assert_eq!(*status, ExitStatus::Code(3));
        assert!(
            message.trim_end().ends_with("error: the real reason"),
            "lost the end of stderr: {}",
            &message[message.len().saturating_sub(200)..]
        );
        assert!(message.len() <= 4 * 1024 + 256, "{} bytes", message.len());
    }

    #[tokio::test]
    async fn a_diagnostic_past_the_output_cap_still_trips_the_tripwire_and_reaches_the_journal() {
        let directory = TempDir::new().expect("a temporary directory");
        let root = directory.path().join("logs");
        let script = "head -c 8192 /dev/zero | tr '\\0' 'e' >&2
echo >&2
echo 'API Error: 429 Too Many Requests' >&2
exit 0";
        let settings = settings(&directory, script)
            .with_output_limit(4 * 1024)
            .with_tripwire(|line| line.contains("429"))
            .with_log(&root);
        let provider = CliProvider::agent(AgentKind::Claude, settings);

        let execution = execute(&provider, &[Message::user("hi")]).await;

        assert!(
            execution
                .failure
                .as_deref()
                .is_some_and(|failure| failure.contains("429 Too Many Requests")),
            "{:?}",
            execution.failure
        );
        let journal = std::fs::read_to_string(&execution.log.clone().expect("logs").events)
            .expect("the journal");
        assert!(
            journal
                .lines()
                .any(|line| line.contains("stderr_line") && line.contains("429 Too Many Requests")),
            "{journal}"
        );
    }

    #[test]
    fn a_failure_keeps_only_the_last_whole_lines_of_stderr() {
        let lines: Vec<String> = (0..500)
            .map(|number| format!("diagnostic line {number}"))
            .collect();

        let rendered = report("the turn failed", &lines.join("\n"));

        let (message, tail) = rendered
            .split_once(STDERR_HEADING)
            .expect("the agent's words, then its stderr");
        assert_eq!(message, "the turn failed");
        assert!(tail.len() <= 1024, "{} bytes", tail.len());
        assert!(tail.ends_with("diagnostic line 499"), "{tail}");
        assert!(
            tail.lines()
                .all(|line| lines.iter().any(|whole| whole == line)),
            "a line was cut short: {tail}"
        );
    }

    #[test]
    fn a_failure_whose_cut_falls_between_lines_keeps_the_line_after_it() {
        let first = "a".repeat(1000);
        let kept = format!("{}\n{}", "b".repeat(500), "c".repeat(523));

        let rendered = report("the turn failed", &format!("{first}\n{kept}"));

        assert_eq!(kept.len(), 1024);
        assert_eq!(
            rendered.split_once(STDERR_HEADING),
            Some(("the turn failed", kept.as_str()))
        );
    }

    #[test]
    fn a_failure_cuts_one_long_stderr_line_between_characters() {
        let rendered = report("the turn failed", &"\u{2014}".repeat(1000));

        let (_, tail) = rendered
            .split_once(STDERR_HEADING)
            .expect("the agent's words, then its stderr");
        assert!(!tail.is_empty() && tail.len() <= 1024);
        assert!(
            tail.chars().all(|character| character == '\u{2014}'),
            "{tail}"
        );
    }

    /// An agent's own report can run to several lines. None of them may read
    /// as stderr, and no line of stderr as the agent's own words.
    #[test]
    fn a_failure_of_several_lines_stays_apart_from_the_stderr_after_it() {
        let words = "tool call error: tool call failed for `docs/echo`\n\nCaused by:\n    \
                     timed out awaiting tools/call after 2s";
        let stderr = "2026-09-23T07:43:43Z WARN codex_mcp: docs: 401 Unauthorized";

        let rendered = report(words, stderr);

        assert_eq!(rendered.split_once(STDERR_HEADING), Some((words, stderr)));
    }

    #[test]
    fn a_failure_with_nothing_on_stderr_is_the_agents_words_alone() {
        assert_eq!(report("the turn failed", " \n\t"), "the turn failed");
    }

    #[tokio::test]
    async fn a_reported_failure_names_the_agents_words_before_the_end_of_its_stderr() {
        let directory = TempDir::new().expect("a temporary directory");
        let script = r#"echo 'retrying the request' >&2
echo 'the credential was revoked' >&2
echo '{"type":"result","subtype":"error_during_execution","is_error":true,"result":"Invalid API key provided"}'"#;
        let provider = CliProvider::agent(AgentKind::Claude, settings(&directory, script));

        let error = run(&provider, &[Message::user("hi")])
            .await
            .expect_err("a failure");

        let ProviderError::Agent { message, .. } = &error else {
            panic!("expected the agent's own failure, got {error:?}");
        };
        assert_eq!(
            message.split_once(STDERR_HEADING),
            Some((
                "Invalid API key provided",
                "retrying the request\nthe credential was revoked"
            ))
        );
    }

    /// A caller tells a run stopped at the output cap from any other failure
    /// by the wording it starts with, and the agent is stopped at once rather
    /// than left to fill its pipe until the timeout.
    #[tokio::test]
    async fn an_answer_past_the_output_cap_stops_the_run_in_the_wording_callers_match() {
        let directory = TempDir::new().expect("a temporary directory");
        let script = r#"
while :; do
  echo '{"type":"assistant","message":{"content":[{"type":"text","text":"xxxxxxxxxxxxxxxx"}]}}'
done
"#;
        let settings = settings(&directory, script).with_output_limit(4 * 1024);
        let provider = CliProvider::agent(AgentKind::Claude, settings);

        let error = run(&provider, &[Message::user("hi")])
            .await
            .expect_err("a failure");

        let ProviderError::Malformed { message, .. } = &error else {
            panic!("expected the overflow, got {error:?}");
        };
        assert!(message.starts_with(PROSE_EXCEEDED), "{message}");
        assert!(message.contains("4096"), "{message}");
    }

    /// What a call was made with reaches no caller as prose, so calls whose
    /// arguments add up to more than the cap still end in an answer.
    #[tokio::test]
    async fn tool_arguments_past_the_output_cap_do_not_end_its_turn() {
        let directory = TempDir::new().expect("a temporary directory");
        let script = r#"
for part in 1 2 3 4 5 6 7 8; do
  printf '{"type":"assistant","message":{"id":"msg_%s","type":"message","content":[{"type":"tool_use","id":"toolu_%s","name":"Write","input":{"file_path":"/w/part.rs","content":"' "$part" "$part"
  head -c 1024 /dev/zero | tr '\0' 'x'
  printf '"}}]}}\n'
done
echo '{"type":"assistant","message":{"content":[{"type":"text","text":"Wrote every part."}]}}'
echo '{"type":"result","subtype":"success","is_error":false}'
"#;
        let provider = CliProvider::agent(
            AgentKind::Claude,
            settings(&directory, script).with_output_limit(4 * 1024),
        );

        let execution = execute(&provider, &[Message::user("Write every part.")]).await;

        assert_eq!(execution.stdout.tools.len(), 8);
        let arguments: usize = execution
            .stdout
            .tools
            .iter()
            .map(|call| call.function.arguments.len())
            .sum();
        assert!(arguments > 4 * 1024, "{arguments} bytes of arguments");
        let completion = provider.assemble(execution).expect("an answer");
        assert_eq!(
            completion.message.content.as_deref(),
            Some("Wrote every part.")
        );
    }

    #[tokio::test]
    async fn a_descendant_holding_the_output_open_is_reaped_after_the_agent_exits() {
        let directory = TempDir::new().expect("a temporary directory");
        let started = directory.path().join("started");
        let script = format!(
            r#"touch '{}'
sleep 120 &
echo '{{"type":"assistant","message":{{"content":[{{"type":"text","text":"done"}}]}}}}'
echo '{{"type":"result","subtype":"success","is_error":false}}'
"#,
            started.display()
        );
        let provider = CliProvider::agent(AgentKind::Claude, settings(&directory, &script));
        let messages = [Message::user("hi")];

        let (completion, waited) = timed(run(&provider, &messages), &started).await;

        assert_eq!(
            completion.expect("an answer").message.content.as_deref(),
            Some("done")
        );
        assert!(waited < PATIENCE, "waited {waited:?} on a straggler");
    }

    #[tokio::test]
    async fn a_cancelled_run_takes_its_process_tree_with_it() {
        let directory = TempDir::new().expect("a temporary directory");
        let marker = directory.path().join("group");
        let script = format!("sleep 120 &\necho $$ > '{}'\nwait", marker.display());
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
            () = tokio::time::sleep(PATIENCE) => panic!("the agent never started"),
            pid = started => pid,
        };

        let started = Instant::now();
        while !running(leader).is_empty() && started.elapsed() < PATIENCE {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(
            running(leader).is_empty(),
            "the agent's group outlived the cancelled run: {:?}",
            running(leader)
        );
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
        assert!(
            document["mcpServers"]["appwrite"]["env"]["APPWRITE_API_KEY"]
                .as_str()
                .is_some_and(|value| value.starts_with("${ABNEGATE_MCP_")),
            "{document}"
        );
        assert!(!path.exists(), "the MCP config outlived the run");
    }

    /// A caller that configured servers chose which ones load. With every
    /// one of them refused there is no file to attach, and the run still
    /// loads strictly, so the CLI never falls back to the repository's own
    /// `.mcp.json`.
    #[tokio::test]
    async fn a_run_whose_every_server_is_refused_loads_no_server_of_the_clis_own() {
        let directory = TempDir::new().expect("a temporary directory");
        let captured = directory.path().join("arguments");
        let script = format!(
            r#"printf '%s\n' "$@" > '{captured}'
echo '{{"type":"result","subtype":"success","is_error":false}}'"#,
            captured = captured.display(),
        );
        let settings = settings(&directory, &script).with_mcp_server(
            "linear",
            McpServer::remote("https://mcp.linear.app/mcp")
                .with_header("Authorization", "Bearer ${LINEAR_TOKEN}"),
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
        assert!(
            arguments.contains(&"--strict-mcp-config".to_string()),
            "{arguments:?}"
        );
        assert!(
            !arguments.contains(&"--mcp-config".to_string()),
            "{arguments:?}"
        );
        assert!(
            !arguments
                .iter()
                .any(|argument| argument.starts_with("mcp__")),
            "{arguments:?}"
        );
    }

    /// A stand-in for Codex that records its arguments and environment, then
    /// answers as `codex exec --json` does.
    fn recording_codex(arguments: &Path, environment: &Path) -> String {
        format!(
            r#"printf '%s\n' "$@" > '{arguments}'
env > '{environment}'
echo '{{"type":"thread.started","thread_id":"t1"}}'
echo '{{"type":"item.completed","item":{{"id":"item_1","type":"agent_message","text":"done"}}}}'
echo '{{"type":"turn.completed","usage":{{"input_tokens":1,"output_tokens":1}}}}'"#,
            arguments = arguments.display(),
            environment = environment.display(),
        )
    }

    fn recorded_arguments(path: &Path) -> Vec<String> {
        std::fs::read_to_string(path)
            .expect("the captured arguments")
            .lines()
            .map(str::to_string)
            .collect()
    }

    /// The `mcp_servers.<name>` override Codex was given, if it was given one.
    fn codex_override<'a>(arguments: &'a [String], name: &str) -> Option<&'a str> {
        let prefix = format!("mcp_servers.{name}=");
        arguments
            .windows(2)
            .find(|pair| pair[0] == "-c" && pair[1].starts_with(&prefix))
            .map(|pair| pair[1].as_str())
    }

    const CODEX_STRICT_MCP: [&str; 5] = [
        "--ignore-user-config",
        "--disable",
        "apps",
        "--disable",
        "plugins",
    ];

    /// Codex is given each server as an override of its own configuration,
    /// loads no server of the user's, a plugin's or an app's beside it, and
    /// is handed a stdio server's literal value only in a generated variable
    /// its override names, never on its command line or under the value's
    /// own name.
    #[tokio::test]
    async fn a_codex_run_is_given_its_servers_strictly_with_no_secret_on_its_command_line() {
        const LITERAL: &str = "glsa-realsecret-codex";
        let directory = TempDir::new().expect("a temporary directory");
        let captured = directory.path().join("arguments");
        let recorded = directory.path().join("environment");
        let settings = settings(&directory, &recording_codex(&captured, &recorded))
            .with_mcp_server(
                "grafana",
                McpServer::command("uvx", ["mcp-grafana"])
                    .with_environment("GRAFANA_TOKEN", LITERAL)
                    .with_tools(["list_datasources"]),
            );
        let provider = CliProvider::agent(AgentKind::Codex, settings);

        let completion = run(&provider, &[Message::user("hi")])
            .await
            .expect("an answer");

        assert_eq!(completion.message.content.as_deref(), Some("done"));
        let arguments = recorded_arguments(&captured);
        assert_eq!(&arguments[..3], ["exec", "--json", "--skip-git-repo-check"]);
        assert_eq!(&arguments[3..8], CODEX_STRICT_MCP);
        assert!(
            !arguments.iter().any(|argument| argument.contains(LITERAL)),
            "{arguments:?}"
        );
        let grafana = codex_override(&arguments, "grafana").expect("a grafana override");
        assert!(grafana.contains(r#""command" = "/bin/sh""#), "{grafana}");
        assert!(
            grafana.contains(r#""enabled_tools" = ["list_datasources"]"#),
            "{grafana}"
        );
        let child = recorded_environment(&recorded);
        let (variable, _) = child
            .iter()
            .find(|(_, value)| value.as_str() == LITERAL)
            .expect("the literal reaches the child");
        assert!(variable.starts_with("ABNEGATE_MCP_"), "{variable}");
        assert!(
            grafana.contains(&format!(r#""env_vars" = ["{variable}"]"#)),
            "{grafana}"
        );
        assert!(!child.contains_key("GRAFANA_TOKEN"), "{child:?}");
    }

    /// A remote server's reference resolves to the secret bound to it, which
    /// reaches Codex only in the generated variable its `env_http_headers`
    /// names, never under its own name, and never reaches a log.
    #[tokio::test]
    async fn a_secret_bound_to_a_remote_server_reaches_codex_and_never_a_log() {
        const TOKEN: &str = "lin-api-codex-marker-7b21";
        let directory = TempDir::new().expect("a temporary directory");
        let root = directory.path().join("logs");
        let captured = directory.path().join("arguments");
        let recorded = directory.path().join("environment");
        let settings = settings(&directory, &recording_codex(&captured, &recorded))
            .with_log(&root)
            .with_mcp_server(
                "linear",
                McpServer::remote("https://mcp.linear.app/mcp")
                    .with_header("Authorization", "Bearer ${LINEAR_TOKEN}")
                    .with_secret("LINEAR_TOKEN", TOKEN),
            );
        let provider = CliProvider::agent(AgentKind::Codex, settings);

        let execution = execute(&provider, &[Message::user("hi")]).await;

        let arguments = recorded_arguments(&captured);
        assert!(
            !arguments.iter().any(|argument| argument.contains(TOKEN)),
            "{arguments:?}"
        );
        let linear = codex_override(&arguments, "linear").expect("a linear override");
        let child = recorded_environment(&recorded);
        let variable = linear
            .split_once(r#""env_http_headers" = { "Authorization" = ""#)
            .and_then(|(_, rest)| rest.split_once('"'))
            .map(|(variable, _)| variable)
            .expect("a generated variable");
        assert_eq!(
            child.get(variable).map(String::as_str),
            Some(format!("Bearer {TOKEN}").as_str())
        );
        assert!(!child.contains_key("LINEAR_TOKEN"), "{child:?}");
        let files = execution.log.clone().expect("log files");
        for path in [&files.stdout, &files.stderr, &files.events] {
            let contents = std::fs::read_to_string(path).expect("a log file");
            assert!(
                !contents.contains(TOKEN),
                "{} leaked the token: {contents}",
                path.display()
            );
        }
    }

    /// A caller that configured servers chose which ones load, so a Codex
    /// run whose every server is refused still loads none of the user's.
    #[tokio::test]
    async fn a_codex_run_whose_every_server_is_refused_loads_no_server_of_its_own() {
        let directory = TempDir::new().expect("a temporary directory");
        let captured = directory.path().join("arguments");
        let recorded = directory.path().join("environment");
        let settings = settings(&directory, &recording_codex(&captured, &recorded))
            .with_mcp_server(
                "events",
                McpServer::remote("https://mcp.example.com/sse").with_transport(McpTransport::Sse),
            );
        let provider = CliProvider::agent(AgentKind::Codex, settings);

        run(&provider, &[Message::user("hi")])
            .await
            .expect("an answer");

        let arguments = recorded_arguments(&captured);
        assert_eq!(&arguments[3..8], CODEX_STRICT_MCP);
        assert!(
            !arguments
                .iter()
                .any(|argument| argument.starts_with("mcp_servers.")),
            "{arguments:?}"
        );
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

    /// A proxy URL can carry a password or a token, and the agent can print
    /// anything its environment holds.
    #[tokio::test]
    async fn a_proxy_credential_never_reaches_a_log() {
        const NAME: &str = "provider::tests::a_proxy_credential_never_reaches_a_log";
        let proxies = [
            (
                "HTTPS_PROXY",
                "http://user:hunter2seventeen@proxy.internal:3128",
            ),
            ("HTTP_PROXY", "http://TOKENVALUE12345@proxy"),
        ];
        if delegated(NAME, &proxies).await {
            return;
        }
        let directory = TempDir::new().expect("a temporary directory");
        let root = directory.path().join("logs");
        let script = r#"
printf '{"type":"assistant","message":{"content":[{"type":"text","text":"via %s and %s"}]}}\n' "$HTTPS_PROXY" "$HTTP_PROXY"
echo "proxies $HTTPS_PROXY $HTTP_PROXY" >&2
echo '{"type":"result","subtype":"success","is_error":false}'
"#;
        let settings = settings(&directory, script)
            .with_log(&root)
            .with_proxy_variables();
        let provider = CliProvider::agent(AgentKind::Claude, settings);

        let execution = execute(&provider, &[Message::user("hi")]).await;

        let files = execution.log.clone().expect("log files");
        for path in [&files.stdout, &files.stderr, &files.events] {
            let contents = std::fs::read_to_string(path).expect("a log file");
            for secret in ["hunter2seventeen", "TOKENVALUE12345"] {
                assert!(
                    !contents.contains(secret),
                    "{} leaked {secret}: {contents}",
                    path.display()
                );
            }
        }
        assert!(
            !execution.stderr.contains("TOKENVALUE12345"),
            "{}",
            execution.stderr
        );
        assert_eq!(
            std::fs::read_to_string(&files.stdout).expect("the prose log"),
            "via [REDACTED] and [REDACTED]",
            "the agent was not handed both proxies"
        );
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

    /// A stand-in agent's script that writes `lines` as its output.
    fn replaying(directory: &TempDir, lines: &[String]) -> String {
        let recording = directory.path().join("recording.jsonl");
        std::fs::write(&recording, format!("{}\n", lines.join("\n"))).expect("the recording");
        format!("cat '{}'", recording.display())
    }

    /// A stand-in agent that replays `stream` as its output.
    fn replayed(directory: &TempDir, stream: &str) -> CliSettings {
        let lines: Vec<String> = stream.lines().map(str::to_string).collect();
        settings(directory, &replaying(directory, &lines))
    }

    fn blocking<T>(work: impl Future<Output = T>) -> T {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a runtime")
            .block_on(work)
    }

    #[tokio::test]
    async fn a_turn_the_account_cannot_fund_fails_in_claudes_words_and_not_as_a_rate_limit() {
        for (stream, failure) in [
            (
                include_str!("../tests/fixtures/claude/model-requires-usage-credits.jsonl"),
                format!(
                    "{CREDITS_REQUIRED}: Fable 5.1 requires usage credits. Switch to another model to continue."
                ),
            ),
            (
                include_str!("../tests/fixtures/claude/overage-included-window-spent.jsonl"),
                format!(
                    "{CREDITS_REQUIRED}: You've reached your Fable limit. Switch to another model to continue."
                ),
            ),
            (
                include_str!("../tests/fixtures/claude/long-context-credits-required.jsonl"),
                format!(
                    "{LONG_CONTEXT_CREDITS_REQUIRED}: API Error: Usage credits required for 1M context \u{b7} turn on usage credits at claude.ai/settings/usage?from=cc_cli_limit_message (they take effect in a new session)"
                ),
            ),
        ] {
            let directory = TempDir::new().expect("a temporary directory");
            let provider = CliProvider::agent(AgentKind::Claude, replayed(&directory, stream));

            let error = run(&provider, &[Message::user("Review the change.")])
                .await
                .expect_err("a refused turn");

            let rendered = error.to_string();
            assert_eq!(rendered, format!("claude: {failure}"));
            assert!(
                !rendered.to_ascii_lowercase().contains("rate limit"),
                "{rendered}"
            );
        }
    }

    /// claude hands a subagent's refusal to the main agent as the result of
    /// the call that started it, and the turn goes on to the main agent's
    /// answer.
    #[tokio::test]
    async fn a_subagents_refusal_leaves_the_turn_to_the_main_agents_answer() {
        const ANSWER: &str = "The subagent could not run on this account, so the review is mine.";
        let stream = [
            json!({"type": "assistant", "message": {"content": [{"type": "tool_use", "id": "toolu_01Agent", "name": "Agent", "input": {"description": "Ask for a review", "prompt": "Review the change.", "model": "fable"}}]}, "parent_tool_use_id": null}),
            json!({"type": "rate_limit_event", "rate_limit_info": {"status": "rejected", "overageStatus": "rejected", "overageDisabledReason": "overage_not_provisioned", "isUsingOverage": false, "errorCode": "credits_required"}}),
            json!({"type": "assistant", "message": {"model": "<synthetic>", "content": [{"type": "text", "text": "Fable 5.1 requires usage credits. Switch to another model to continue."}]}, "parent_tool_use_id": "toolu_01Agent", "is_api_error_message": true, "api_error": "model_requires_usage_credits"}),
            json!({"type": "assistant", "message": {"content": [{"type": "text", "text": ANSWER}]}, "parent_tool_use_id": null}),
            json!({"type": "result", "subtype": "success", "is_error": false, "result": ANSWER}),
        ]
        .map(|line| line.to_string());
        let directory = TempDir::new().expect("a temporary directory");
        let provider = CliProvider::agent(
            AgentKind::Claude,
            settings(&directory, &replaying(&directory, &stream)),
        );

        let completion = run(
            &provider,
            &[Message::user("Ask for a review of the change.")],
        )
        .await
        .expect("the main agent's answer");

        assert_eq!(completion.message.content.as_deref(), Some(ANSWER));
    }

    /// claude refuses a subagent's request past the plan's window on an
    /// event that names no agent, and the turn goes on to the main agent's
    /// answer.
    #[tokio::test]
    async fn a_subagents_refused_window_leaves_the_turn_to_the_main_agents_answer() {
        const ANSWER: &str = "Opus is past its weekly limit, so the review is mine.";
        let stream = [
            json!({"type": "assistant", "message": {"content": [{"type": "tool_use", "id": "toolu_01Agent", "name": "Agent", "input": {"description": "Ask Opus", "prompt": "Review the change.", "model": "opus"}}]}, "parent_tool_use_id": null}),
            json!({"type": "rate_limit_event", "rate_limit_info": {"status": "rejected", "resetsAt": 1_790_208_000, "rateLimitType": "seven_day_opus", "isUsingOverage": false}}),
            json!({"type": "assistant", "message": {"model": "<synthetic>", "content": [{"type": "text", "text": "You've hit your Opus limit \u{b7} resets Mon 9am"}]}, "parent_tool_use_id": "toolu_01Agent", "error": "rate_limit", "is_api_error_message": true}),
            json!({"type": "assistant", "message": {"content": [{"type": "text", "text": ANSWER}]}, "parent_tool_use_id": null}),
            json!({"type": "result", "subtype": "success", "is_error": false, "result": ANSWER}),
        ]
        .map(|line| line.to_string());
        let directory = TempDir::new().expect("a temporary directory");
        let provider = CliProvider::agent(
            AgentKind::Claude,
            settings(&directory, &replaying(&directory, &stream)),
        );

        let completion = run(
            &provider,
            &[Message::user("Ask Opus to review the change.")],
        )
        .await
        .expect("the main agent's answer");

        assert_eq!(completion.message.content.as_deref(), Some(ANSWER));
    }

    /// A turn usage credits carry past the plan's window is logged once, with
    /// the window, and never with its credential.
    #[test]
    fn a_turn_on_usage_credits_is_logged_once_with_its_window() {
        const TOKEN: &str = concat!(
            "sk-ant-",
            "api03-",
            "BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB"
        );
        let stream = [
            json!({"type": "rate_limit_event", "rate_limit_info": {"status": "rejected", "rateLimitType": "five_hour", "overageStatus": "allowed", "isUsingOverage": true}}),
            json!({"type": "assistant", "message": {"content": [{"type": "text", "text": "Reviewed."}]}}),
            json!({"type": "rate_limit_event", "rate_limit_info": {"status": "rejected", "rateLimitType": "five_hour", "overageStatus": "allowed_warning", "isUsingOverage": true}}),
            json!({"type": "result", "subtype": "success", "is_error": false}),
        ]
        .map(|line| line.to_string());
        let directory = TempDir::new().expect("a temporary directory");
        let provider = CliProvider::agent(
            AgentKind::Claude,
            settings(&directory, &replaying(&directory, &stream))
                .with_credential(Credential::key("ANTHROPIC_API_KEY", TOKEN)),
        );

        let (completion, logged) =
            captured_logs(|| blocking(run(&provider, &[Message::user("Review the change.")])));

        assert_eq!(
            completion
                .expect("a turn on usage credits")
                .message
                .content
                .as_deref(),
            Some("Reviewed.")
        );
        let credited: Vec<&str> = logged
            .lines()
            .filter(|line| line.contains("usage credits"))
            .collect();
        assert_eq!(credited.len(), 1, "{logged}");
        assert!(credited[0].contains("INFO"), "{logged}");
        assert!(credited[0].contains("five_hour"), "{logged}");
        assert!(!logged.contains(TOKEN), "{logged}");
    }

    #[test]
    fn a_turn_inside_the_plans_window_logs_no_usage_credits() {
        let directory = TempDir::new().expect("a temporary directory");
        let provider = CliProvider::agent(AgentKind::Claude, settings(&directory, CLAUDE_SESSION));

        let (completion, logged) =
            captured_logs(|| blocking(run(&provider, &[Message::user("What does a.rs do?")])));

        completion.expect("an answer");
        assert!(!logged.contains("usage credits"), "{logged}");
    }

    /// A signed-out claude speaks its refusal as assistant text first, and
    /// only the result after it says the turn failed.
    #[tokio::test]
    async fn a_refusal_spoken_before_it_is_declared_still_fails_the_turn() {
        const REFUSAL: &str = "Not logged in \u{b7} Please run /login";
        let stream = [
            json!({"type": "assistant", "message": {"id": "msg_1", "type": "message", "role": "assistant", "model": "claude-opus-4", "content": [{"type": "text", "text": REFUSAL}], "stop_reason": null, "usage": {"input_tokens": 1, "output_tokens": 1}}, "session_id": "s1"}),
            json!({"type": "result", "subtype": "success", "is_error": true, "terminal_reason": "api_error", "result": REFUSAL}),
        ]
        .map(|line| line.to_string());
        let directory = TempDir::new().expect("a temporary directory");
        let provider = CliProvider::agent(
            AgentKind::Claude,
            settings(&directory, &replaying(&directory, &stream)),
        );

        let error = run(&provider, &[Message::user("hi")])
            .await
            .expect_err("a signed-out agent to fail the turn");

        assert!(
            error.to_string().contains(REFUSAL),
            "lost the agent's wording: {error}"
        );
    }
}
