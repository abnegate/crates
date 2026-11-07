//! A provider backed by a coding agent CLI run as a child process.

use std::io::Write;
use std::path::PathBuf;
use std::process::Stdio;

use abnegate_exec::Marker;
use abnegate_llm::Capabilities;
use abnegate_llm::Completion;
use abnegate_llm::CompletionProvider;
use abnegate_llm::CompletionRequest;
use abnegate_llm::ExitStatus;
use abnegate_llm::Message;
use abnegate_llm::ProviderError;
use abnegate_llm::ProviderKind;
use async_trait::async_trait;
use futures::StreamExt;
use serde_json::json;
use tempfile::NamedTempFile;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::sync::mpsc;
use tokio::sync::watch;
use tokio::task::JoinSet;
use tokio::time::Instant;

use crate::STDERR_HEADING;
use crate::agent_stream::AgentStream;
use crate::attachments::Attachments;
use crate::diagnostics;
use crate::diagnostics::Diagnostics;
use crate::environment::Environment;
use crate::event::AgentEvent;
use crate::execution::Execution;
use crate::execution_error::ExecutionError;
use crate::input::Input;
use crate::kind::AgentKind;
use crate::log::EXECUTION_LOG_PREVIEW_LIMIT;
use crate::log::ExecutionLogFiles;
use crate::log::Journal;
use crate::log::Record;
use crate::log::Sink;
use crate::log::tail_preview;
use crate::mcp::McpAttachment;
use crate::reader::Reader;
use crate::reaper::Reaper;
use crate::rollout::Rollout;
use crate::run::Run;
use crate::scrubber::Scrubber;
use crate::settings::CliSettings;
use crate::spawned::Spawned;
use crate::stdout_parse_result::StdoutParseResult;

const NO_DIAGNOSTICS: &str = "the agent produced no diagnostics";
const UNFINISHED: &str = "the agent exited without completing its event stream";
const INSTRUCTIONS_PREFIX: &str = "instructions-";
const INSTRUCTIONS_SUFFIX: &str = ".md";
const STOPPING: &str = "its registry has stopped every run";

/// Drives a coding agent CLI as a completion provider.
///
/// The agent runs its own tool loop, so [`CompletionRequest::tools`] is not
/// forwarded: the agent has its own tools and no way to call the caller's.
/// What comes back is the agent's [reply](StdoutParseResult::reply): its
/// final answer, without the narration it streamed on the way there, when it
/// reports the two apart, as Claude does, or else all its prose. Tool
/// activity is still parsed,
/// so a stream full of it cannot derail the run, but it is not returned as
/// tool calls: the agent already executed them, and replaying them through the
/// caller's registry would run each one a second time.
///
/// [`CliProvider::stream`] is the primary form: a turn takes minutes, and its
/// events exist long before the agent exits. [`CompletionProvider::complete`]
/// is that stream read to its end.
///
/// # Confinement
///
/// The agent does not run under `abnegate-exec`'s sandbox. That sandbox denies
/// all network access and grants `process-exec` for a single literal command,
/// and a coding agent needs the network to reach its own API and forks a tree
/// of helper processes to do its work. It does reuse that crate's
/// process-group termination, [`Marker`] sweeps,
/// [`GroupRegistry`](abnegate_exec::GroupRegistry) and output
/// caps, so a run that times out or fails takes the agent's whole process
/// tree with it, and a host can stop every run at once; and it is given only
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
    /// A provider that runs `agent` under `settings`, reporting itself as
    /// `name` in its errors and logs.
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

    /// The settings every run is started with.
    pub fn settings(&self) -> &CliSettings {
        &self.settings
    }

    /// Whether this provider's runs wait on the agent's background work: only
    /// Claude reports any.
    pub(crate) fn awaits_background(&self) -> bool {
        self.settings.await_background && self.agent == AgentKind::Claude
    }

    /// Run the agent once and report everything it did, with `label` naming
    /// the run in its execution logs.
    ///
    /// A run that exits on its own, or is stopped once its output settled it,
    /// is an [`Execution`] however it ended; judging it is left to the caller,
    /// as [`CompletionProvider::complete`] does. Its
    /// [`stdout`](Execution::stdout) keeps a Claude turn's final
    /// [answer](StdoutParseResult::answer) apart from the prose streamed on
    /// the way to it. What fails here is only what
    /// leaves nothing to judge: settings the agent cannot honour, an agent
    /// that cannot be started, one that outlives its timeout with no answer
    /// given, and output that cannot be read as the agent's stream.
    ///
    /// What the agent left behind is stopped as
    /// [`CliSettings::cleanup`] says: by default, a run that succeeded
    /// leaves it running, and any other takes the agent's whole process
    /// group, and everything carrying the run's
    /// [marker](CliSettings::marker), with it. A run that fails here still
    /// reports where its logs are, with every line read before it failed
    /// flushed to them.
    ///
    /// The conversation reaches the agent as
    /// [`transcript::render`](crate::transcript::render) flattens it, every
    /// message under its role's label, byte for byte as
    /// [`CliProvider::execute_prompt`] would send that rendering, unless a
    /// message carries [images](Message::images). Then Claude reads the
    /// conversation's [parts](crate::transcript::parts) as one stream-json
    /// message (`--input-format stream-json`), the images as base64 or url
    /// content blocks, and Codex reads the text on stdin, each image marked
    /// where it was sent, and opens the images from files it is pointed at
    /// with `--image`, which are removed when the run ends. An image on an
    /// earlier message that cannot be sent, such as a link Codex cannot open
    /// or one past what one Claude request carries (10 MB of base64 an
    /// image, 100 images, and 127 MB in all, its text sized as the JSON it is
    /// sent as), is noted in its place instead; once one image is crowded
    /// out, every older one is too. One on the last user message that cannot
    /// be sent fails the run before the agent starts, as a
    /// [`ProviderError::Agent`] naming the agent and the [`ImageError`] that
    /// says why. The images are decoded, encoded and saved on the blocking
    /// pool, off the runtime.
    ///
    /// [`ImageError`]: crate::ImageError
    pub async fn execute(
        &self,
        request: CompletionRequest<'_>,
        label: &str,
    ) -> Result<Execution, ExecutionError> {
        let input = self.input(request.messages).await?;
        self.start(Some(request.model), input, label, None)
            .await?
            .finish()
            .await
    }

    /// [`CliProvider::execute`] with `prompt` written to the agent's stdin
    /// byte for byte: no role label, no trimming and no escaping, so Claude
    /// reads a leading slash command such as `/issue ENG-42` as one. A
    /// `model` that is `None` or blank leaves the agent on its own default.
    ///
    /// Since nothing in `prompt` is escaped, a line in it that reads as a
    /// role label such as `Assistant:` reaches the agent as one. Text the
    /// caller does not control belongs in a conversation sent through
    /// [`CliProvider::execute`], which
    /// [`transcript::render`](crate::transcript::render) escapes.
    pub async fn execute_prompt(
        &self,
        model: Option<&str>,
        prompt: impl Into<String>,
        label: &str,
    ) -> Result<Execution, ExecutionError> {
        self.start(model, Input::raw(prompt.into()), label, None)
            .await?
            .finish()
            .await
    }

    /// Start the agent and yield its events as it emits them, each the moment
    /// its line is read. A Claude turn that succeeds yields its final answer
    /// as an [`AgentEvent::Answer`] just before the [`AgentEvent::Finished`]
    /// that ends it, after the [`AgentEvent::Text`] it streamed on the way.
    /// A Codex agent that exits by itself, having named its session, yields
    /// the [`AgentEvent::Occupancy`] its rollout reports last, before the
    /// run's error when it failed; a run that timed out, was stopped, or
    /// tripped [`CliSettings::tripwire`] reads no rollout.
    ///
    /// The agent is spawned before this returns, so settings it cannot
    /// honour and an agent that cannot be started are this call's error,
    /// never a stream that fails on its first item. The run is the one
    /// [`CompletionProvider::complete`] makes, logged under the provider's
    /// name: its timeout counts from the spawn and covers the whole stream,
    /// and once the agent has exited, or been stopped, the stream ends, with
    /// the error `complete` would have returned when the run failed. A
    /// failure the agent reported ends it that way, carrying the end of the
    /// agent's stderr, and nothing it emitted after that failure is yielded.
    /// A turn a usage limit refused ends it the same way, as a
    /// [`ProviderError::Limited`] whose message carries the end of the
    /// agent's stderr too; a Claude stream that ends on a refused window with
    /// no line after it ends in a [`ProviderError::Agent`] in the same words.
    ///
    /// The run is waited on by a task of its own from the moment this
    /// returns, whether or not the stream is read: the agent is stopped at
    /// its timeout, or once its output settles the run, even while nobody
    /// polls the stream, and what it emitted waits in the stream, followed by
    /// the run's error, until it is read. Dropping the stream while the agent
    /// still runs, read or not, stops it, every process it forked and
    /// everything carrying the run's [marker](CliSettings::marker).
    ///
    /// The conversation reaches the agent as [`CliProvider::execute`] hands
    /// it over, its images included.
    ///
    /// Call it within a Tokio runtime, which runs that task.
    pub async fn stream(
        &self,
        request: CompletionRequest<'_>,
    ) -> Result<AgentStream, ProviderError> {
        let input = self.input(request.messages).await?;
        self.stream_input(Some(request.model), input).await
    }

    /// [`CliProvider::stream`] with `prompt` written to the agent's stdin as
    /// [`CliProvider::execute_prompt`] writes it, byte for byte and with
    /// nothing escaped.
    ///
    /// Call it within a Tokio runtime, which runs the task waiting on the run.
    pub async fn stream_prompt(
        &self,
        model: Option<&str>,
        prompt: impl Into<String>,
    ) -> Result<AgentStream, ProviderError> {
        self.stream_input(model, Input::raw(prompt.into())).await
    }

    async fn stream_input(
        &self,
        model: Option<&str>,
        input: Input,
    ) -> Result<AgentStream, ProviderError> {
        let (forward, mut events) = mpsc::unbounded_channel();
        let run = self.start(model, input, &self.name, Some(forward)).await?;
        let name = self.name.clone();
        let mut finishing = JoinSet::new();
        finishing.spawn(run.finish());
        Ok(Box::pin(async_stream::try_stream! {
            let joined = loop {
                tokio::select! {
                    biased;
                    Some(joined) = finishing.join_next() => break joined,
                    Some(event) = events.recv() => yield event,
                }
            };
            while let Ok(event) = events.try_recv() {
                yield event;
            }
            let ended = match joined {
                Ok(ended) => ended,
                Err(error) if error.is_panic() => std::panic::resume_unwind(error.into_panic()),
                Err(error) => Err(ExecutionError::new(ProviderError::malformed(&name, error), None)),
            };
            assemble(&name, ended?)?;
        }))
    }

    /// `messages` as this provider's agent is handed them, or the reason an
    /// image the turn was sent with cannot reach it.
    async fn input(&self, messages: &[Message]) -> Result<Input, ExecutionError> {
        Input::conversation(self.agent, messages)
            .await
            .map_err(|error| {
                ExecutionError::new(ProviderError::agent(&self.name, &error.to_string()), None)
            })
    }

    /// Spawn the agent on `input` with the reading of its output under way,
    /// forwarding each event read to `events` when given.
    async fn start(
        &self,
        model: Option<&str>,
        input: Input,
        label: &str,
        events: Option<mpsc::UnboundedSender<AgentEvent>>,
    ) -> Result<Run, ExecutionError> {
        let model = model.map(str::trim).filter(|model| !model.is_empty());
        let mcp = self
            .attach()
            .map_err(|error| ExecutionError::new(error, None))?;
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
        let mut options = self
            .agent
            .options(&self.settings, &attachments)
            .map_err(|error| ExecutionError::new(error, None))?;
        if let Some(effort) = self.settings.effort {
            options.extend(self.agent.effort_arguments(model, effort));
        }
        let arguments =
            self.agent
                .invocation_with(model, options, self.settings.session.as_ref(), &input);
        let Input {
            stdin: prompt,
            directory: images,
            ..
        } = input;
        let marker = self
            .settings
            .marker
            .as_deref()
            .map(Marker::new)
            .transpose()
            .map_err(|error| ExecutionError::new(ProviderError::config(error), None))?;
        let host = |name: &str| std::env::var_os(name);
        let environment = Environment::new(self.agent, &self.settings, mcp.as_ref(), &host);
        let rollout = match self.agent {
            AgentKind::Codex => Rollout::locate(
                &environment,
                &host,
                self.settings.working_directory.as_deref(),
                model,
            ),
            AgentKind::Claude => None,
        };
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
                    "model": model,
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

        let Some(reaper) = Reaper::admit(self.settings.registry.clone(), marker) else {
            journal
                .append(Record::SpawnFailed, json!({ "error": STOPPING }))
                .await;
            return Err(ExecutionError::new(
                ProviderError::unavailable(&self.name, &self.executable(), STOPPING),
                files,
            ));
        };
        let child = match self
            .command(&arguments, &environment, reaper.marker())
            .spawn()
        {
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
        let mut spawned = Spawned::new(reaper, child);
        journal
            .append(Record::Spawned, json!({ "pid": spawned.child.id() }))
            .await;

        // The prompt is written concurrently with reading the reply: a prompt
        // larger than the pipe buffer blocks until the child drains it, and a
        // child nobody reads from blocks on its own output first.
        let stdin = spawned.child.stdin.take();
        let writer = tokio::spawn(async move {
            if let Some(mut stdin) = stdin {
                let _ = stdin.write_all(prompt.as_bytes()).await;
                let _ = stdin.shutdown().await;
            }
        });

        let forward = events.clone();
        let (verdicts, settled) = mpsc::unbounded_channel();
        let (draining, started) = watch::channel(false);
        let prose = Sink::open(files.as_ref().map(|files| files.stdout.as_path())).await;
        let raw = Sink::open(files.as_ref().map(|files| files.stderr.as_path())).await;
        let reader = tokio::spawn(
            Reader::new(
                self.agent,
                self.settings.line_limit,
                self.settings.output_limit,
                journal.clone(),
                prose,
                scrubber.clone(),
                verdicts.clone(),
                started.clone(),
            )
            .forwarding(events)
            .awaiting_background(self.awaits_background())
            .printing(self.settings.tool_output_limit)
            .in_directory(self.settings.working_directory.clone())
            .run(spawned.child.stdout.take()),
        );
        let diagnostics = tokio::spawn(
            Diagnostics::new(
                self.settings.line_limit,
                self.settings.output_limit,
                journal.clone(),
                raw,
                scrubber.clone(),
                self.settings.tripwire.clone(),
                verdicts,
                started,
            )
            .run(spawned.child.stderr.take()),
        );

        let Spawned { reaper, child } = spawned;
        Ok(Run {
            name: self.name.clone(),
            label: label.to_string(),
            timeout: self.settings.timeout,
            deadline: Instant::now().checked_add(self.settings.timeout),
            cleanup: self.settings.cleanup,
            journal,
            files,
            reaper,
            child,
            writer,
            reader,
            diagnostics,
            settled,
            await_background: self.awaits_background(),
            draining,
            rollout,
            forward,
            _mcp: mcp,
            _instructions: instructions,
            _images: images,
        })
    }

    /// Render the MCP servers to attach, or attach none when rendering fails:
    /// the run still loads strictly, with no server at all, and without its
    /// MCP tools can still answer, and fails loudly on its own if it truly
    /// needed them.
    fn attach(&self) -> Result<Option<McpAttachment>, ProviderError> {
        let mcp = &self.settings.mcp;
        if mcp.is_empty() {
            return Ok(None);
        }
        if let Some((name, refusal)) = mcp.unattached_required(self.agent) {
            return Err(ProviderError::config(format!(
                "the required MCP server {name} cannot attach to {}: {refusal}",
                self.agent
            )));
        }
        let attachment = match mcp.render(self.agent) {
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
            Err(error) if mcp.requires(self.agent) => {
                return Err(ProviderError::config(format!(
                    "could not render the MCP config of a run with a required server: {error}"
                )));
            }
            Err(error) => {
                tracing::warn!(
                    provider = %self.name,
                    %error,
                    "could not render the MCP config; continuing without MCP servers"
                );
                None
            }
        };
        Ok(attachment)
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

    fn command(
        &self,
        arguments: &[String],
        environment: &Environment,
        marker: Option<&Marker>,
    ) -> Command {
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
        if let Some(marker) = marker {
            marker.apply(&mut command);
        }

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
        let mut events = self.stream(request).await?;
        let mut stdout = StdoutParseResult::default();
        while let Some(event) = events.next().await {
            stdout.record(event?);
        }
        Ok(completion(&self.name, stdout))
    }
}

/// Judge `execution` as [`CompletionProvider::complete`] does, and return
/// the answer when it succeeded.
fn assemble(name: &str, execution: Execution) -> Result<Completion, ProviderError> {
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
        let reported = report(message, &stderr);
        return Err(match stdout.limit {
            Some(limit) => ProviderError::limited(name, limit.with_message(reported)),
            None => ProviderError::agent(name, &reported),
        });
    }

    let unstopped = stopped.is_none();
    match (failure.or(stopped), stdout.finished) {
        (Some(reason), false) => return Err(ProviderError::agent(name, &reason)),
        _ if unstopped && status != ExitStatus::Code(0) => {
            let message = if stderr.trim().is_empty() {
                NO_DIAGNOSTICS.to_string()
            } else {
                tail_preview(stderr.trim_end(), EXECUTION_LOG_PREVIEW_LIMIT)
            };
            return Err(ProviderError::exit(name, status, &message));
        }
        // A clean exit is not proof of a finished turn. An agent killed
        // between its last token and its result line still exits zero.
        (None, false) => return Err(ProviderError::agent(name, UNFINISHED)),
        (_, true) => {}
    }

    Ok(completion(name, stdout))
}

/// The answer `stdout` amounts to, from the provider called `name`.
fn completion(name: &str, stdout: StdoutParseResult) -> Completion {
    Completion::new(name, Message::assistant(stdout.reply()))
        .with_usage(stdout.usage)
        .with_finish_reason(stdout.finish_reason)
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
    use abnegate_llm::ReasoningEffort;
    use abnegate_llm::RequestOptions;
    use abnegate_llm::WindowKind;
    use abnegate_secret::SecretValue;
    use futures::StreamExt;
    use serde_json::Value;
    use serde_json::json;
    use tempfile::TempDir;

    use super::CliProvider;
    use super::assemble;
    use super::report;
    use crate::PROSE_EXCEEDED;
    use crate::STDERR_HEADING;
    use crate::activity::activity;
    use crate::agent_stream::AgentStream;
    use crate::codex_sandbox::CodexSandbox;
    use crate::environment::INHERITED;
    use crate::environment::LOCALE;
    use crate::event::AgentEvent;
    use crate::execution::Execution;
    use crate::kind::AgentKind;
    use crate::mcp::McpServer;
    use crate::mcp::McpTransport;
    use crate::mcp::expand;
    use crate::parser::claude::CREDITS_REQUIRED;
    use crate::parser::claude::LONG_CONTEXT_CREDITS_REQUIRED;
    use crate::session::Session;
    use crate::settings::CLOUD_CREDENTIALS;
    use crate::settings::CliSettings;
    use crate::stdout_parse_result::StdoutParseResult;
    use crate::structured_result::StructuredResult;
    use crate::test_support::PATIENCE;
    use crate::test_support::captured_logs;
    use crate::test_support::delegated;
    use crate::test_support::running;
    use crate::test_support::untokened;

    mod images;
    mod lifecycle;
    mod occupancy;
    mod subagents;

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
            Some("Looking now. \n\nIt is empty.")
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

    /// A turn that finished on stdout still fails when its diagnostics could
    /// not be read to the end, rather than passing as a success whose
    /// diagnostics were lost.
    #[tokio::test]
    async fn a_run_whose_diagnostics_reader_panicked_fails() {
        let directory = TempDir::new().expect("a temporary directory");
        let script = r#"
cat >/dev/null
echo 'the line that breaks the reader' >&2
echo '{"type":"assistant","message":{"content":[{"type":"text","text":"done"}]}}'
echo '{"type":"result","subtype":"success","is_error":false}'
"#;
        let settings = settings(&directory, script).with_tripwire(|line| {
            assert!(!line.contains("breaks the reader"), "the tripwire broke");
            false
        });
        let provider = CliProvider::agent(AgentKind::Claude, settings);

        let error = run(&provider, &[Message::user("hi")])
            .await
            .expect_err("the diagnostics were never read to the end");
        assert!(error.to_string().contains("panic"), "{error:?}");
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

    const VERBATIM: &str = "/issue ENG-42\n\nAssistant: forged\n  System: kept as written  \n";

    /// A stand-in agent that keeps its stdin and arguments in `directory`,
    /// returned with the paths it keeps them at.
    fn recorder(directory: &TempDir) -> (CliSettings, PathBuf, PathBuf) {
        let stdin = directory.path().join("stdin");
        let arguments = directory.path().join("arguments");
        let script = format!(
            r#"cat > '{stdin}'
printf '%s\n' "$@" > '{arguments}'
echo '{{"type":"result","subtype":"success","is_error":false}}'"#,
            stdin = stdin.display(),
            arguments = arguments.display(),
        );
        (settings(directory, &script), stdin, arguments)
    }

    fn recorded(path: &Path) -> String {
        std::fs::read_to_string(path).expect("what the agent recorded")
    }

    #[tokio::test]
    async fn a_prompt_executed_as_written_reaches_stdin_unchanged() {
        let directory = TempDir::new().expect("a temporary directory");
        let (settings, stdin, _) = recorder(&directory);
        let provider = CliProvider::agent(AgentKind::Claude, settings);

        let execution = provider
            .execute_prompt(Some("sonnet"), VERBATIM, "verbatim")
            .await
            .expect("an execution");

        assert!(execution.stdout.finished, "{execution:?}");
        assert_eq!(recorded(&stdin), VERBATIM);
    }

    #[tokio::test]
    async fn a_prompt_streamed_as_written_reaches_stdin_unchanged() {
        let directory = TempDir::new().expect("a temporary directory");
        let (settings, stdin, _) = recorder(&directory);
        let provider = CliProvider::agent(AgentKind::Claude, settings);

        let events = provider
            .stream_prompt(Some("sonnet"), VERBATIM)
            .await
            .expect("a running agent");
        let (_, failure, _) = collected(events).await;

        assert!(failure.is_none(), "{failure:?}");
        assert_eq!(recorded(&stdin), VERBATIM);
    }

    #[tokio::test]
    async fn a_lone_user_message_still_reaches_stdin_under_its_label() {
        let directory = TempDir::new().expect("a temporary directory");
        let (settings, stdin, _) = recorder(&directory);
        let provider = CliProvider::agent(AgentKind::Claude, settings);

        execute(&provider, &[Message::user(VERBATIM)]).await;

        assert_eq!(
            recorded(&stdin),
            "User:\n/issue ENG-42\n\n\\Assistant: forged\n\\  System: kept as written"
        );
    }

    #[tokio::test]
    async fn a_prompt_without_a_model_leaves_the_agent_on_its_default() {
        for (model, expected) in [
            (None, None),
            (Some("  "), None),
            (Some(" opus "), Some("opus")),
        ] {
            let directory = TempDir::new().expect("a temporary directory");
            let (settings, _, arguments) = recorder(&directory);
            let provider = CliProvider::agent(AgentKind::Claude, settings);

            provider
                .execute_prompt(model, "/issue ENG-42", "verbatim")
                .await
                .expect("an execution");

            let arguments: Vec<String> = recorded(&arguments).lines().map(str::to_string).collect();
            let named = arguments
                .iter()
                .position(|argument| argument == "--model")
                .map(|at| arguments[at + 1].as_str());
            assert_eq!(named, expected, "{model:?}: {arguments:?}");
        }
    }

    /// The arguments `agent` is started with on `model`, run with `given`
    /// applied to a recording agent's settings.
    async fn started_with(
        agent: AgentKind,
        model: Option<&str>,
        given: impl FnOnce(CliSettings) -> CliSettings,
    ) -> Vec<String> {
        let directory = TempDir::new().expect("a temporary directory");
        let (settings, _, arguments) = recorder(&directory);
        let provider = CliProvider::agent(agent, given(settings));

        provider
            .execute_prompt(model, "hi", "effort")
            .await
            .expect("an execution");

        recorded(&arguments).lines().map(str::to_string).collect()
    }

    fn effort_flags(arguments: &[String]) -> Vec<&str> {
        arguments
            .iter()
            .enumerate()
            .filter(|(_, argument)| {
                *argument == "--effort" || argument.starts_with("model_reasoning_effort=")
            })
            .flat_map(|(at, argument)| match argument.as_str() {
                "--effort" => [Some("--effort"), arguments.get(at + 1).map(String::as_str)],
                _ => [
                    at.checked_sub(1)
                        .and_then(|before| arguments.get(before))
                        .map(String::as_str),
                    Some(argument),
                ],
            })
            .flatten()
            .collect()
    }

    #[tokio::test]
    async fn the_effort_a_run_is_given_reaches_the_agent_when_its_model_takes_it() {
        for (agent, model, effort, expected) in [
            (
                AgentKind::Claude,
                Some("opus"),
                ReasoningEffort::Max,
                &["--effort", "max"][..],
            ),
            (
                AgentKind::Claude,
                None,
                ReasoningEffort::XHigh,
                &["--effort", "xhigh"],
            ),
            (
                AgentKind::Claude,
                Some("  "),
                ReasoningEffort::Low,
                &["--effort", "low"],
            ),
            (
                AgentKind::Codex,
                Some("gpt-6-sol"),
                ReasoningEffort::Ultra,
                &["-c", r#"model_reasoning_effort="ultra""#],
            ),
            (AgentKind::Claude, Some("haiku"), ReasoningEffort::Low, &[]),
            (
                AgentKind::Claude,
                Some("claude-opus-4-8"),
                ReasoningEffort::Max,
                &[],
            ),
            (AgentKind::Claude, Some("opus"), ReasoningEffort::Auto, &[]),
            (AgentKind::Codex, None, ReasoningEffort::Off, &[]),
            (
                AgentKind::Codex,
                Some("qwen2.5:7b-instruct"),
                ReasoningEffort::High,
                &[],
            ),
        ] {
            let arguments =
                started_with(agent, model, |settings| settings.with_effort(effort)).await;

            assert_eq!(
                effort_flags(&arguments),
                expected,
                "{agent} on {model:?} at {effort:?}: {arguments:?}"
            );
            if agent == AgentKind::Codex {
                assert!(
                    !arguments.contains(&"--effort".to_string()),
                    "{arguments:?}"
                );
            }
        }
    }

    #[tokio::test]
    async fn a_run_given_no_effort_sends_none() {
        for agent in AgentKind::ALL {
            let model = agent.models().first().copied();
            let arguments = started_with(*agent, model, |settings| settings).await;

            assert!(
                effort_flags(&arguments).is_empty(),
                "{agent}: {arguments:?}"
            );
        }
    }

    /// The crate never second-guesses the model a caller names: a local
    /// model Codex reaches through its own provider configuration is passed
    /// as written, and only the effort it is not known to take is held back.
    #[tokio::test]
    async fn a_local_codex_model_still_reaches_the_agent_verbatim() {
        let arguments = started_with(
            AgentKind::Codex,
            Some(" qwen2.5:7b-instruct "),
            |settings| settings.with_effort(ReasoningEffort::High),
        )
        .await;

        let model = arguments
            .iter()
            .position(|argument| argument == "--model")
            .map(|at| arguments[at + 1].as_str());
        assert_eq!(model, Some("qwen2.5:7b-instruct"), "{arguments:?}");
        assert!(effort_flags(&arguments).is_empty(), "{arguments:?}");
    }

    #[tokio::test]
    async fn a_resumed_codex_run_keeps_its_effort() {
        let id = "019b2c41-0000-7000-8000-000000000001";
        let arguments = started_with(AgentKind::Codex, Some("gpt-6-sol"), |settings| {
            settings
                .with_session(Session::resumed(id))
                .with_sandbox(CodexSandbox::WorkspaceWrite)
                .with_effort(ReasoningEffort::Ultra)
        })
        .await;

        assert_eq!(
            arguments,
            [
                "exec",
                "--json",
                "--skip-git-repo-check",
                "-c",
                r#"sandbox_mode="workspace-write""#,
                "-c",
                r#"model_reasoning_effort="ultra""#,
                "--model",
                "gpt-6-sol",
                "resume",
                id,
                "-",
            ]
        );
    }

    #[tokio::test]
    async fn a_read_only_claude_run_carries_its_effort() {
        let arguments = started_with(AgentKind::Claude, Some("sonnet"), |settings| {
            settings.read_only().with_effort(ReasoningEffort::High)
        })
        .await;

        assert_eq!(
            effort_flags(&arguments),
            ["--effort", "high"],
            "{arguments:?}"
        );
        assert!(
            arguments.contains(&"--restricted".to_string()),
            "{arguments:?}"
        );
    }

    #[tokio::test]
    async fn a_run_given_an_effort_refuses_a_caller_flag_setting_another() {
        for (agent, arguments) in [
            (AgentKind::Claude, &["--effort", "low"][..]),
            (AgentKind::Codex, &["-c", "model_reasoning_effort=low"]),
        ] {
            let directory = TempDir::new().expect("a temporary directory");
            let (settings, _, recorded) = recorder(&directory);
            let provider = CliProvider::agent(
                agent,
                settings
                    .with_arguments(arguments.iter().copied())
                    .with_effort(ReasoningEffort::High),
            );

            let error = provider
                .execute_prompt(None, "hi", "effort")
                .await
                .expect_err("a refusal");

            assert!(
                matches!(&*error.error, ProviderError::Config { detail, .. } if detail.contains("effort")),
                "{agent}: {error:?}"
            );
            assert!(!recorded.exists(), "{agent} was started anyway");
        }
    }

    #[tokio::test]
    async fn a_prompt_reaches_codex_on_stdin_unchanged() {
        let directory = TempDir::new().expect("a temporary directory");
        let (settings, stdin, _) = recorder(&directory);
        let provider = CliProvider::agent(AgentKind::Codex, settings);

        provider
            .execute_prompt(None, VERBATIM, "verbatim")
            .await
            .expect("an execution");

        assert_eq!(recorded(&stdin), VERBATIM);
    }

    #[tokio::test]
    async fn the_journal_names_the_model_the_agent_was_given() {
        for (model, expected) in [
            (None, Value::Null),
            (Some("  "), Value::Null),
            (Some(" opus "), json!("opus")),
        ] {
            let directory = TempDir::new().expect("a temporary directory");
            let (settings, _, _) = recorder(&directory);
            let provider = CliProvider::agent(
                AgentKind::Claude,
                settings.with_log(directory.path().join("logs")),
            );

            let execution = provider
                .execute_prompt(model, "/issue ENG-42", "verbatim")
                .await
                .expect("an execution");

            let journal = recorded(&execution.log.expect("log files").events);
            let initialized: Value = journal
                .lines()
                .map(|line| serde_json::from_str::<Value>(line).expect("a JSON line"))
                .find(|entry| entry["event"] == "execution_initialized")
                .expect("an initialized record");
            assert_eq!(
                initialized["data"]["model"], expected,
                "{model:?}: {journal}"
            );
        }
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
        let script = r#"
echo '{"type":"system","subtype":"init","session_id":"s-9"}'
echo '{"type":"result","subtype":"error_max_turns","is_error":true,"result":"","session_id":"s-9"}'
"#;
        let provider = CliProvider::agent(AgentKind::Claude, settings(&directory, script));

        let execution = execute(&provider, &[Message::user("hi")]).await;

        assert_eq!(execution.stdout.failure.as_deref(), Some("error_max_turns"));
        assert_eq!(execution.stdout.session.as_deref(), Some("s-9"));
        assert!(execution.stdout.finished);
    }

    /// A turn whose narration goes before its final answer.
    const NARRATION_THEN_RESULT: &str =
        include_str!("../tests/fixtures/claude/narration-then-result.jsonl");
    const NARRATION: &str = "#42 returned 500 — LIVE FAIL, retrying.";
    const FINAL_REPORT: &str = "- #42 fix uploads LIVE PASS\nDEPLOY_QA_VERDICT: ALL_VERIFIED";

    /// A turn that ran out of turns, which says so only in its `errors`.
    const ERROR_MAX_TURNS: &str =
        include_str!("../tests/fixtures/claude/error-max-turns-with-errors.jsonl");
    const MAX_TURNS: &str = "Reached maximum number of turns (30)";
    const MAX_TURNS_NARRATION: &str = "Let me check.";

    #[tokio::test]
    async fn a_completion_is_the_final_answer_without_the_narration() {
        let directory = TempDir::new().expect("a temporary directory");
        let provider = CliProvider::agent(
            AgentKind::Claude,
            replayed(&directory, NARRATION_THEN_RESULT),
        );

        let completion = run(&provider, &[Message::user("Check the deploy.")])
            .await
            .expect("an answer");

        assert_eq!(completion.message.content.as_deref(), Some(FINAL_REPORT));
    }

    #[tokio::test]
    async fn a_streamed_turn_yields_its_final_answer_just_before_it_finishes() {
        let directory = TempDir::new().expect("a temporary directory");
        let provider = CliProvider::agent(
            AgentKind::Claude,
            replayed(&directory, NARRATION_THEN_RESULT),
        );

        let (delivered, failure, _) =
            collected(streamed(&provider, &[Message::user("Check the deploy.")]).await).await;

        assert!(failure.is_none(), "{failure:?}");
        let texts: Vec<&str> = delivered
            .iter()
            .filter_map(|event| match event {
                AgentEvent::Text(text) => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            texts,
            [NARRATION.to_string(), format!("\n\n{FINAL_REPORT}")]
        );
        let [.., AgentEvent::Answer(answer), AgentEvent::Finished { .. }] = delivered.as_slice()
        else {
            panic!("expected the answer, then the finish: {delivered:?}");
        };
        assert_eq!(answer, FINAL_REPORT);
    }

    #[tokio::test]
    async fn an_execution_keeps_the_final_answer_apart_from_the_narration() {
        let directory = TempDir::new().expect("a temporary directory");
        let provider = CliProvider::agent(
            AgentKind::Claude,
            replayed(&directory, NARRATION_THEN_RESULT).with_log(directory.path().join("logs")),
        );

        let execution = execute(&provider, &[Message::user("Check the deploy.")]).await;

        assert_eq!(execution.stdout.answer.as_deref(), Some(FINAL_REPORT));
        assert_eq!(
            execution.stdout.text,
            format!("{NARRATION}\n\n{FINAL_REPORT}")
        );
        assert_eq!(execution.stdout.reply(), FINAL_REPORT);
        let journal = recorded(&execution.log.expect("log files").events);
        let completed: Value = journal
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).expect("a JSON line"))
            .find(|entry| entry["event"] == "process_completed")
            .expect("a completed record");
        assert_eq!(completed["data"]["has_answer"], true, "{journal}");
    }

    #[tokio::test]
    async fn a_turn_out_of_turns_fails_in_the_errors_claude_gave() {
        let directory = TempDir::new().expect("a temporary directory");
        let provider = CliProvider::agent(AgentKind::Claude, replayed(&directory, ERROR_MAX_TURNS));
        let messages = [Message::user("Fix the tests.")];

        let error = run(&provider, &messages).await.expect_err("a failure");
        assert!(
            matches!(&error, ProviderError::Agent { message, .. } if message == MAX_TURNS),
            "{error:?}"
        );

        let (delivered, failure, _) = collected(streamed(&provider, &messages).await).await;
        assert!(
            matches!(&failure, Some(ProviderError::Agent { message, .. }) if message == MAX_TURNS),
            "{failure:?}"
        );
        assert!(
            !delivered
                .iter()
                .any(|event| matches!(event, AgentEvent::Answer(_))),
            "a failed turn yielded an answer: {delivered:?}"
        );

        let execution = execute(&provider, &messages).await;
        assert_eq!(execution.stdout.failure.as_deref(), Some(MAX_TURNS));
        assert_eq!(execution.stdout.answer, None);
        assert_eq!(execution.stdout.text, MAX_TURNS_NARRATION);
        assert_eq!(execution.stdout.turns, Some(30));
    }

    #[tokio::test]
    async fn a_failed_exit_keeps_an_unflagged_final_answer_out_of_the_error() {
        let directory = TempDir::new().expect("a temporary directory");
        let lines: Vec<String> = NARRATION_THEN_RESULT.lines().map(str::to_string).collect();
        let script = format!("{}\nexit 1", replaying(&directory, &lines));
        let provider = CliProvider::agent(AgentKind::Claude, settings(&directory, &script));

        let error = run(&provider, &[Message::user("Check the deploy.")])
            .await
            .expect_err("a run whose process failed is no answer");

        assert!(
            matches!(
                &error,
                ProviderError::Exit {
                    status: ExitStatus::Code(1),
                    ..
                }
            ),
            "{error:?}"
        );
        assert!(!error.to_string().contains(FINAL_REPORT), "{error}");
        assert!(!error.to_string().contains(NARRATION), "{error}");
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
        assert!(execution.stdout.limit.is_some(), "{:?}", execution.stdout);
        let error = assemble(provider.name(), execution).expect_err("a failure");
        let ProviderError::Limited { limit, .. } = &error else {
            panic!("expected the limit, got {error:?}");
        };
        assert_eq!(
            limit.message,
            "rate limit reached (five_hour, rejected): You have hit your limit"
        );
        assert_eq!(
            limit.resets_at,
            chrono::DateTime::from_timestamp(1_772_096_400, 0)
        );
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
            Some("Looking now. \n\nIt is empty.")
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

    async fn streamed(provider: &CliProvider, messages: &[Message]) -> AgentStream {
        provider
            .stream(request(messages))
            .await
            .expect("a running agent")
    }

    /// Every event `events` yields, and the error it ended with, if any, with
    /// how many events followed that error.
    async fn collected(mut events: AgentStream) -> (Vec<AgentEvent>, Option<ProviderError>, usize) {
        let mut delivered = Vec::new();
        let mut failure = None;
        let mut after = 0;
        while let Some(event) = events.next().await {
            match event {
                Ok(event) if failure.is_none() => delivered.push(event),
                Ok(_) => after += 1,
                Err(error) => failure = Some(error),
            }
        }
        (delivered, failure, after)
    }

    #[tokio::test]
    async fn a_missing_agent_fails_the_stream_before_it_starts() {
        let provider = CliProvider::agent(
            AgentKind::Claude,
            CliSettings::default().with_executable("/nonexistent/abnegate/claude"),
        );

        let Err(error) = provider.stream(request(&[Message::user("hi")])).await else {
            panic!("a missing agent was started");
        };

        let ProviderError::Unavailable { executable, .. } = &error else {
            panic!("expected an unavailable agent, got {error:?}");
        };
        assert_eq!(executable, "/nonexistent/abnegate/claude");
    }

    #[tokio::test]
    async fn settings_the_agent_cannot_honour_fail_the_stream_before_it_starts() {
        let directory = TempDir::new().expect("a temporary directory");
        let marker = directory.path().join("started");
        let settings =
            settings(&directory, &format!("touch '{}'", marker.display())).with_schema("{}");
        let provider = CliProvider::agent(AgentKind::Codex, settings);

        let Err(error) = provider.stream(request(&[Message::user("hi")])).await else {
            panic!("codex was started with a claude-only setting");
        };

        assert!(
            matches!(error, ProviderError::Unsupported { .. }),
            "{error:?}"
        );
        assert!(!marker.exists(), "the agent was started anyway");
    }

    #[tokio::test]
    async fn a_streamed_turn_yields_every_event_in_order_and_ends_cleanly() {
        let directory = TempDir::new().expect("a temporary directory");
        let provider = CliProvider::agent(AgentKind::Claude, settings(&directory, CLAUDE_SESSION));
        let messages = [Message::user("What does a.rs do?")];

        let (delivered, failure, _) = collected(streamed(&provider, &messages).await).await;

        assert!(failure.is_none(), "{failure:?}");
        let texts: Vec<&str> = delivered
            .iter()
            .filter_map(|event| match event {
                AgentEvent::Text(text) => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(texts, ["Looking now. ", "\n\nIt is empty."]);
        let tool = delivered
            .iter()
            .position(
                |event| matches!(event, AgentEvent::Tool(call) if call.function.name == "Read"),
            )
            .expect("the tool call");
        let second = delivered
            .iter()
            .position(|event| matches!(event, AgentEvent::Text(text) if text == "\n\nIt is empty."))
            .expect("the second text");
        assert!(tool < second, "{delivered:?}");
        assert!(
            delivered
                .iter()
                .any(|event| matches!(event, AgentEvent::Finished { .. })),
            "{delivered:?}"
        );

        let folded: StdoutParseResult = delivered.into_iter().collect();
        let executed = execute(&provider, &messages).await.stdout;
        assert_eq!(folded.text, executed.text);
        assert_eq!(folded.usage, executed.usage);
        assert_eq!(folded.finish_reason, executed.finish_reason);
        assert_eq!(folded.session, executed.session);
        assert_eq!(folded.structured, executed.structured);
        assert_eq!(folded.cost, executed.cost);
    }

    /// The blocks a host's agent writes either side of a call of the host's
    /// own tools are kept apart, and the call itself is left to the host.
    #[tokio::test]
    async fn a_host_agents_answer_keeps_its_blocks_apart() {
        let directory = TempDir::new().expect("a temporary directory");
        let script = r#"
echo '{"type":"assistant","message":{"content":[{"type":"text","text":"I will look first."}]}}'
echo '{"type":"assistant","message":{"content":[{"type":"tool_use","id":"toolu_01","name":"mcp__host__list_files","input":{"path":"."}}]}}'
echo '{"type":"assistant","message":{"content":[{"type":"text","text":"Found it."}]}}'
echo '{"type":"result","subtype":"success","is_error":false}'
"#;
        let provider = CliProvider::agent(AgentKind::Claude, settings(&directory, script));
        let messages = [Message::user("Find it.")];

        let (delivered, failure, _) = collected(streamed(&provider, &messages).await).await;

        assert!(failure.is_none(), "{failure:?}");
        let spoken: String = delivered
            .iter()
            .filter_map(|event| match event {
                AgentEvent::Text(text) => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(spoken, "I will look first.\n\nFound it.");
        let listed: Vec<String> = delivered
            .iter()
            .filter_map(|event| match event {
                AgentEvent::Tool(call) => activity(call, &["mcp__host__"]),
                _ => None,
            })
            .collect();
        assert!(listed.is_empty(), "{listed:?}");

        let folded: StdoutParseResult = delivered.into_iter().collect();
        assert_eq!(folded.text, spoken);
        let completion = run(&provider, &messages).await.expect("an answer");
        assert_eq!(completion.message.content.as_deref(), Some(spoken.as_str()));
    }

    #[tokio::test]
    async fn events_reach_the_consumer_before_the_agent_exits() {
        let directory = TempDir::new().expect("a temporary directory");
        let script = r#"
echo '{"type":"assistant","message":{"content":[{"type":"text","text":"Looking now."}]}}'
sleep 120
echo '{"type":"result","subtype":"success","is_error":false}'
"#;
        let provider = CliProvider::agent(AgentKind::Claude, settings(&directory, script));

        let mut events = streamed(&provider, &[Message::user("What does a.rs do?")]).await;
        let first = tokio::time::timeout(PATIENCE, events.next())
            .await
            .expect("the text to arrive while the agent is still running")
            .expect("an event")
            .expect("a readable event");

        assert!(
            matches!(&first, AgentEvent::Text(text) if text == "Looking now."),
            "expected the agent's text, got {first:?}"
        );
    }

    /// The process id the fake agent writes to `path`, once it has.
    async fn written_pid(path: &Path) -> u32 {
        let started = Instant::now();
        loop {
            if let Some(pid) = std::fs::read_to_string(path)
                .ok()
                .and_then(|written| written.trim().parse().ok())
            {
                return pid;
            }
            assert!(
                started.elapsed() < PATIENCE,
                "the agent never wrote {}",
                path.display()
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// The members of `group` still running once it has had [`PATIENCE`] to
    /// empty, each killed so that no test leaves one behind.
    async fn survivors(group: u32) -> Vec<String> {
        let started = Instant::now();
        while !running(group).is_empty() && started.elapsed() < PATIENCE {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let survivors = running(group);
        for pid in &survivors {
            let _ = std::process::Command::new("kill")
                .args(["-9", pid])
                .status();
        }
        survivors
    }

    /// An agent that writes its group to `leader`, forks a long sleep, says
    /// one thing and then sleeps past every test's patience.
    fn lingering(leader: &Path) -> String {
        format!(
            r#"echo $$ > '{}'
sleep 120 >/dev/null 2>&1 &
echo '{{"type":"assistant","message":{{"content":[{{"type":"text","text":"Working."}}]}}}}'
sleep 120"#,
            leader.display()
        )
    }

    #[tokio::test]
    async fn dropping_the_stream_stops_the_agent_and_what_it_forked() {
        let directory = TempDir::new().expect("a temporary directory");
        let leader = directory.path().join("leader");
        let provider =
            CliProvider::agent(AgentKind::Claude, settings(&directory, &lingering(&leader)));

        let mut events = streamed(&provider, &[Message::user("hi")]).await;
        tokio::time::timeout(PATIENCE, events.next())
            .await
            .expect("the agent's first event")
            .expect("an event")
            .expect("a readable event");
        let group = written_pid(&leader).await;
        assert!(running(group).len() > 1, "the agent never forked");

        drop(events);

        let survivors = survivors(group).await;
        assert!(
            survivors.is_empty(),
            "dropping the stream left {survivors:?} running"
        );
    }

    #[tokio::test]
    async fn dropping_a_stream_never_read_stops_the_agent_and_what_it_forked() {
        let directory = TempDir::new().expect("a temporary directory");
        let leader = directory.path().join("leader");
        let provider =
            CliProvider::agent(AgentKind::Claude, settings(&directory, &lingering(&leader)));

        let events = streamed(&provider, &[Message::user("hi")]).await;
        let group = written_pid(&leader).await;
        let started = Instant::now();
        while running(group).len() < 2 && started.elapsed() < PATIENCE {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        drop(events);

        let survivors = survivors(group).await;
        assert!(
            survivors.is_empty(),
            "dropping the unread stream left {survivors:?} running"
        );
    }

    #[tokio::test]
    async fn a_stream_nobody_reads_still_stops_its_agent_at_the_timeout() {
        let directory = TempDir::new().expect("a temporary directory");
        let leader = directory.path().join("leader");
        let settings =
            settings(&directory, &lingering(&leader)).with_timeout(Duration::from_secs(1));
        let provider = CliProvider::agent(AgentKind::Claude, settings);

        let mut events = streamed(&provider, &[Message::user("hi")]).await;
        let group = written_pid(&leader).await;
        let survivors = survivors(group).await;
        assert!(
            survivors.is_empty(),
            "the unread stream left {survivors:?} running past its timeout"
        );

        let first = events.next().await.expect("an event");
        assert!(
            matches!(&first, Ok(AgentEvent::Text(text)) if text == "Working."),
            "{first:?}"
        );
        let second = events.next().await.expect("the run's error");
        assert!(
            matches!(second, Err(ProviderError::Timeout { .. })),
            "{second:?}"
        );
        assert!(events.next().await.is_none(), "the stream went on");
    }

    #[tokio::test]
    async fn the_timeout_covers_the_whole_stream_not_each_event() {
        let directory = TempDir::new().expect("a temporary directory");
        let script = r#"
while :; do
  echo '{"type":"assistant","message":{"content":[{"type":"text","text":"."}]}}'
  sleep 0.1
done
"#;
        let settings = settings(&directory, script).with_timeout(Duration::from_secs(1));
        let provider = CliProvider::agent(AgentKind::Claude, settings);

        let started = Instant::now();
        let (delivered, failure, after) =
            collected(streamed(&provider, &[Message::user("hi")]).await).await;

        assert!(
            matches!(failure, Some(ProviderError::Timeout { .. })),
            "expected a timeout, got {failure:?}"
        );
        assert!(!delivered.is_empty(), "nothing arrived before the timeout");
        assert_eq!(after, 0, "the stream went on past its error");
        assert!(started.elapsed() < Duration::from_secs(30));
    }

    #[tokio::test]
    async fn a_failed_turn_ends_the_stream_in_the_agents_words_and_the_end_of_its_stderr() {
        let directory = TempDir::new().expect("a temporary directory");
        let script = r#"
echo '{"type":"assistant","message":{"content":[{"type":"text","text":"Halfway there."}]}}'
echo 'could not refresh the session' >&2
echo '{"type":"result","subtype":"success","is_error":true,"terminal_reason":"api_error","result":"Not logged in · Please run /login"}'
echo '{"type":"assistant","message":{"content":[{"type":"text","text":"After the failure."}]}}'
"#;
        let provider = CliProvider::agent(AgentKind::Claude, settings(&directory, script));

        let (delivered, failure, after) =
            collected(streamed(&provider, &[Message::user("hi")]).await).await;

        assert!(
            matches!(delivered.as_slice(), [AgentEvent::Text(text)] if text == "Halfway there."),
            "{delivered:?}"
        );
        assert_eq!(after, 0, "the stream went on past its error");
        let failure = failure.expect("a failed turn ends in an error");
        assert!(
            matches!(failure, ProviderError::Agent { .. }),
            "{failure:?}"
        );
        let message = failure.to_string();
        assert!(
            message.contains("Not logged in · Please run /login"),
            "lost the agent's words: {message}"
        );
        let (_, stderr) = message
            .split_once(STDERR_HEADING)
            .expect("the end of stderr");
        assert!(
            stderr.contains("could not refresh the session"),
            "lost the end of stderr: {message}"
        );
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
                    || INHERITED.contains(&name.as_str())
                    || AgentKind::Claude.configuration().contains(&name.as_str())
                    || shell.contains(&name.as_str())
                    || LOCALE.contains(&name.as_str())
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

    /// What a server's environment holds that an agent it runs under a
    /// sign-in of the server's choosing may never be handed, its own
    /// sign-in and the agents' stored state included. Every value says
    /// `notreal`, so a leak shows up under any name.
    const SERVER_SECRETS: &[(&str, &str)] = &[
        ("DATABASE_URL", "postgres://app:notrealpassword@db/app"),
        ("JWT_SECRET", "jwt-notrealsecret"),
        ("ENCRYPTION_KEY", "encryption-notrealkey"),
        ("LITELLM_KEY", concat!("sk-", "litellm-notrealkey")),
        ("ANTHROPIC_API_KEY", concat!("sk-ant-", "notreal-key")),
        ("ANTHROPIC_AUTH_TOKEN", "notreal-auth-token"),
        (
            "CLAUDE_CODE_OAUTH_TOKEN",
            concat!("sk-ant-", "oat-notreal-token"),
        ),
        ("ANTHROPIC_BASE_URL", "https://notreal-gateway.internal"),
        ("OPENAI_API_KEY", concat!("sk-", "notreal-openai-key")),
        ("CODEX_API_KEY", "notreal-codex-key"),
        ("CODEX_ACCESS_TOKEN", "notreal-codex-token"),
        ("CLAUDE_CONFIG_DIR", "/home/server/.claude-notreal"),
        ("CODEX_HOME", "/home/server/.codex-notreal"),
        ("CLAUDECODE", "notreal-session"),
        ("CLAUDE_CODE_ENTRYPOINT", "notreal-entrypoint"),
        ("CLAUDE_CODE_USE_BEDROCK", "1"),
        ("AWS_BEARER_TOKEN_BEDROCK", "notreal-bedrock-token"),
        ("AWS_SECRET_ACCESS_KEY", "notreal-aws-secret"),
        ("CLAUDE_CODE_USE_VERTEX", "1"),
        ("ANTHROPIC_VERTEX_PROJECT_ID", "notreal-project"),
        (
            "GOOGLE_APPLICATION_CREDENTIALS",
            "/home/server/notreal-gcp.json",
        ),
        ("LC_GITHUB_TOKEN", "notreal-smuggled-token"),
    ];

    /// The server's secrets are set in this test's own child process, and
    /// the caller allows every variable an agent or a cloud provider signs
    /// it in with by name.
    /// Whichever agent runs, with its credential inherited and the host's
    /// withheld, no sign-in and no agent's stored state passes, inheriting
    /// the host's whole environment or not, while the locale does.
    #[tokio::test]
    async fn the_servers_secrets_and_the_agents_own_state_are_never_inherited() {
        const NAME: &str =
            "provider::tests::the_servers_secrets_and_the_agents_own_state_are_never_inherited";
        const LOCALES: &[(&str, &str)] = &[
            ("LC_ALL", "C.UTF-8"),
            ("LC_CTYPE", "UTF-8"),
            ("LC_MESSAGES", "en_GB.UTF-8"),
        ];
        let host: Vec<(&str, &str)> = SERVER_SECRETS.iter().chain(LOCALES).copied().collect();
        if delegated(NAME, &host).await {
            return;
        }
        let directory = TempDir::new().expect("a temporary directory");
        let recorded = directory.path().join("environment");
        let script = recording_environment(&recorded);
        let credentials = AgentKind::ALL
            .iter()
            .flat_map(|agent| agent.credentials())
            .chain(CLOUD_CREDENTIALS)
            .copied();

        for agent in AgentKind::ALL {
            for inherit in [false, true] {
                let mut settings = settings(&directory, &script)
                    .allow(credentials.clone())
                    .withhold_host_credentials();
                if inherit {
                    settings = settings.inherit_environment();
                }
                let provider = CliProvider::agent(*agent, settings);

                execute(&provider, &[Message::user("hi")]).await;

                let child = recorded_environment(&recorded);
                let withheld: Vec<&str> = AgentKind::ALL
                    .iter()
                    .flat_map(|agent| agent.credentials().iter().chain(agent.configuration()))
                    .chain(CLOUD_CREDENTIALS)
                    .chain(agent.scrubbed())
                    .copied()
                    .filter(|name| child.contains_key(*name))
                    .collect();
                assert!(
                    withheld.is_empty(),
                    "{agent}, inheriting {inherit}, was handed the host's {withheld:?}"
                );
                if inherit {
                    continue;
                }
                let carrying: Vec<&String> = child
                    .iter()
                    .filter(|(_, value)| value.contains("notreal"))
                    .map(|(name, _)| name)
                    .collect();
                assert!(
                    carrying.is_empty(),
                    "{agent} was handed {carrying:?} from the host"
                );
                for (name, value) in LOCALES {
                    assert_eq!(
                        child.get(*name).map(String::as_str),
                        Some(*value),
                        "{agent} lost the host's {name}"
                    );
                }
            }
        }
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
        let completion = assemble(provider.name(), execution).expect("an answer");
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
        let completion = assemble(provider.name(), execution).expect("an answer");
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

        let completion = assemble(provider.name(), execution).expect("the finished answer");
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
    async fn output_written_without_end_outside_the_group_ends_the_drain_keeping_the_answer() {
        let directory = TempDir::new().expect("a temporary directory");
        let marker = directory.path().join("escaped");
        let started = directory.path().join("started");
        let script = format!(
            r#"touch '{started}'
echo '{{"type":"assistant","message":{{"content":[{{"type":"text","text":"Kept."}}]}}}}'
echo '{{"type":"result","subtype":"success","is_error":false}}'
perl -e 'use POSIX qw(setsid); setsid(); open(my $file, ">", $ARGV[0]) or die; print $file $$; close($file); $| = 1; print "x" x 65536 while 1' '{marker}' &
while [ ! -s '{marker}' ]; do sleep 0.1; done"#,
            started = started.display(),
            marker = marker.display(),
        );
        let provider = CliProvider::agent(AgentKind::Claude, settings(&directory, &script));
        let messages = [Message::user("hi")];

        let (completion, waited) = timed(run(&provider, &messages), &started).await;

        let escaped = std::fs::read_to_string(&marker).unwrap_or_default();
        let _ = std::process::Command::new("kill")
            .args(["-KILL", escaped.trim()])
            .status();
        assert_eq!(
            completion.expect("an answer").message.content.as_deref(),
            Some("Kept.")
        );
        let bound =
            abnegate_exec::executor::GRACE_PERIOD + crate::drain::CUTOFF + Duration::from_secs(5);
        assert!(
            waited < bound,
            "waited {waited:?}, past the grace and the drain's cutoff, on output written without end outside the group"
        );
    }

    /// claude 2.1.318's own lines for a turn that leaves a command running in
    /// the background: the first result, then a further turn once it ends.
    const BACKGROUND_COMMAND: &str =
        include_str!("../tests/fixtures/claude/background-command.jsonl");

    const FIRST_ANSWER: &str = "STARTED the work is running in the background";
    const LATER_ANSWER: &str = "FINAL after the background work";

    /// claude 2.1.318's own lines for a turn that sets a wakeup, then the turn
    /// the wakeup leads to.
    const SCHEDULED_WAKEUP: &str = include_str!("../tests/fixtures/claude/scheduled-wakeup.jsonl");

    /// claude 2.1.318's own lines for a command that ended while the model was
    /// still writing, so its notice comes before the first result.
    const BACKGROUND_ENDED_FIRST: &str =
        include_str!("../tests/fixtures/claude/background-ended-first.jsonl");

    /// The same for two commands, which claude answers with a turn each, the
    /// first of them an empty result.
    const TWO_ENDED_FIRST: &str =
        include_str!("../tests/fixtures/claude/two-background-ended-first.jsonl");

    /// A stand-in that replays `fixture`, whatever comes after the first
    /// result taking longer than a settled run is given to exit.
    fn replaying_turns(directory: &TempDir, fixture: &str) -> CliSettings {
        replaying_after(directory, fixture, 0)
    }

    /// A stand-in that replays `fixture`, pausing longer than a settled run is
    /// given to exit after its result at `index`, counting from zero, and after
    /// the `init` that begins the next turn when claude wrote it straight after.
    fn replaying_after(directory: &TempDir, fixture: &str, index: usize) -> CliSettings {
        let lines: Vec<&str> = fixture.lines().collect();
        let paused = lines
            .iter()
            .enumerate()
            .filter(|(_, line)| line.contains(r#""type":"result""#))
            .nth(index)
            .expect("the result to pause after")
            .0;
        let begins_next = lines
            .get(paused + 1)
            .is_some_and(|line| line.contains(r#""subtype":"init""#));
        let (started, ended) = lines.split_at(paused + 1 + usize::from(begins_next));
        let path = |name: &str| directory.path().join(name);
        std::fs::write(path("started.jsonl"), started.join("\n") + "\n").expect("the first turns");
        std::fs::write(path("ended.jsonl"), ended.join("\n") + "\n").expect("the later turns");
        let wait = abnegate_exec::executor::GRACE_PERIOD + Duration::from_secs(4);
        let script = format!(
            "cat '{started}'\nsleep {wait}\ncat '{ended}'",
            started = path("started.jsonl").display(),
            wait = wait.as_secs(),
            ended = path("ended.jsonl").display(),
        );
        settings(directory, &script)
    }

    #[tokio::test]
    async fn a_run_awaiting_background_work_reads_the_turn_it_leads_to() {
        let directory = TempDir::new().expect("a temporary directory");
        let provider = CliProvider::agent(
            AgentKind::Claude,
            replaying_turns(&directory, BACKGROUND_COMMAND).await_background(),
        );

        let execution = execute(&provider, &[Message::user("hi")]).await;

        assert_eq!(execution.stopped, None, "the agent was stopped mid-work");
        assert_eq!(execution.status, ExitStatus::Code(0));
        let stdout = &execution.stdout;
        assert!(stdout.text.contains(FIRST_ANSWER), "{stdout:?}");
        assert!(stdout.text.contains(LATER_ANSWER), "{stdout:?}");
    }

    #[tokio::test]
    async fn a_run_awaiting_background_work_reads_the_turn_a_wakeup_leads_to() {
        let directory = TempDir::new().expect("a temporary directory");
        let provider = CliProvider::agent(
            AgentKind::Claude,
            replaying_turns(&directory, SCHEDULED_WAKEUP).await_background(),
        );

        let execution = execute(&provider, &[Message::user("hi")]).await;

        assert_eq!(
            execution.stopped, None,
            "the agent was stopped before waking"
        );
        let stdout = &execution.stdout;
        assert_eq!(
            stdout.text.matches(LATER_ANSWER).count(),
            2,
            "both turns' words: {stdout:?}"
        );
    }

    #[tokio::test]
    async fn a_run_not_awaiting_background_work_stops_after_the_first_result() {
        let directory = TempDir::new().expect("a temporary directory");
        let provider = CliProvider::agent(
            AgentKind::Claude,
            replaying_turns(&directory, BACKGROUND_COMMAND),
        );

        let execution = execute(&provider, &[Message::user("hi")]).await;

        assert!(execution.stopped.is_some(), "the agent was left to run on");
        assert!(!execution.stdout.text.contains(LATER_ANSWER));
    }

    /// An agent that lingers once its work is done is still stopped, which is
    /// what a settled run's cutoff is for.
    #[tokio::test]
    async fn a_run_awaiting_background_work_stops_an_agent_with_none_running() {
        let directory = TempDir::new().expect("a temporary directory");
        let started = directory.path().join("started");
        let script = format!(
            r#"touch '{started}'
echo '{{"type":"system","subtype":"background_tasks_changed","tasks":[{{"task_id":"b1"}}]}}'
echo '{{"type":"system","subtype":"background_tasks_changed","tasks":[]}}'
echo '{{"type":"result","subtype":"success","is_error":false,"result":"Kept."}}'
sleep 120"#,
            started = started.display(),
        );
        let provider = CliProvider::agent(
            AgentKind::Claude,
            settings(&directory, &script).await_background(),
        );

        let (execution, waited) = timed(
            async { execute(&provider, &[Message::user("hi")]).await },
            &started,
        )
        .await;

        assert!(execution.stopped.is_some(), "the agent was never stopped");
        let bound = abnegate_exec::executor::GRACE_PERIOD + Duration::from_secs(5);
        assert!(waited < bound, "waited {waited:?} on an agent with no work");
    }

    #[tokio::test]
    async fn a_failure_settles_a_run_with_background_work_running() {
        let directory = TempDir::new().expect("a temporary directory");
        let started = directory.path().join("started");
        let script = format!(
            r#"touch '{started}'
echo '{{"type":"system","subtype":"background_tasks_changed","tasks":[{{"task_id":"b1"}}]}}'
echo '{{"type":"result","subtype":"error_during_execution","is_error":true,"result":"It broke."}}'
sleep 120"#,
            started = started.display(),
        );
        let provider = CliProvider::agent(
            AgentKind::Claude,
            settings(&directory, &script).await_background(),
        );

        let (execution, waited) = timed(
            async { execute(&provider, &[Message::user("hi")]).await },
            &started,
        )
        .await;

        assert!(execution.stopped.is_some(), "the agent was never stopped");
        assert!(execution.failure.is_some(), "{execution:?}");
        let bound = abnegate_exec::executor::GRACE_PERIOD + Duration::from_secs(5);
        assert!(waited < bound, "waited {waited:?} on a failed agent");
    }

    #[tokio::test]
    async fn background_work_that_ends_before_the_first_result_still_gets_its_turn() {
        let directory = TempDir::new().expect("a temporary directory");
        let provider = CliProvider::agent(
            AgentKind::Claude,
            replaying_turns(&directory, BACKGROUND_ENDED_FIRST).await_background(),
        );

        let execution = execute(&provider, &[Message::user("hi")]).await;

        assert_eq!(execution.stopped, None, "the agent was stopped mid-turn");
        assert!(
            execution.stdout.text.contains(LATER_ANSWER),
            "{execution:?}"
        );
    }

    #[tokio::test]
    async fn every_task_that_ended_before_the_first_result_gets_its_turn() {
        let directory = TempDir::new().expect("a temporary directory");
        let provider = CliProvider::agent(
            AgentKind::Claude,
            replaying_after(&directory, TWO_ENDED_FIRST, 1).await_background(),
        );

        let execution = execute(&provider, &[Message::user("hi")]).await;

        assert_eq!(execution.stopped, None, "stopped on the empty result");
        assert!(
            execution.stdout.text.contains(LATER_ANSWER),
            "{execution:?}"
        );
    }

    /// claude 2.1.318's own lines for a monitor that printed a line and ended
    /// while the model was still writing: its line begins a third turn once the
    /// second has settled the run.
    const MONITOR_ENDED_FIRST: &str =
        include_str!("../tests/fixtures/claude/monitor-ended-first.jsonl");

    #[tokio::test]
    async fn a_monitors_line_after_the_run_settled_still_gets_its_turn() {
        let directory = TempDir::new().expect("a temporary directory");
        let provider = CliProvider::agent(
            AgentKind::Claude,
            replaying_after(&directory, MONITOR_ENDED_FIRST, 1).await_background(),
        );

        let execution = execute(&provider, &[Message::user("hi")]).await;

        assert_eq!(execution.stopped, None, "{execution:?}");
        assert_eq!(
            execution.stdout.text.matches(LATER_ANSWER).count(),
            2,
            "{execution:?}"
        );
        assert_ne!(
            execution.stdout.finish_reason.as_deref(),
            Some(crate::CUT_SHORT)
        );
    }

    /// The tripwire still fails a run whose agent trips it in the grace after
    /// its answer, whether or not the run waits on background work.
    #[tokio::test]
    async fn a_diagnostic_tripped_in_the_grace_still_fails_the_run() {
        let directory = TempDir::new().expect("a temporary directory");
        let script = r#"echo '{"type":"result","subtype":"success","is_error":false,"result":"Done."}'
sleep 1
echo 'API Error: 429 Too Many Requests' >&2
sleep 1
exit 0"#;
        for awaited in [false, true] {
            let mut settings =
                settings(&directory, script).with_tripwire(|line| line.contains("429"));
            if awaited {
                settings = settings.await_background();
            }
            let provider = CliProvider::agent(AgentKind::Claude, settings);

            let execution = execute(&provider, &[Message::user("hi")]).await;

            assert!(
                execution
                    .failure
                    .as_deref()
                    .is_some_and(|failure| failure.contains("429")),
                "awaited {awaited}: {execution:?}"
            );
        }
    }

    /// A trip in the grace still fails the run when a turn begins after it.
    #[tokio::test]
    async fn a_diagnostic_tripped_before_a_reopening_still_fails_the_run() {
        let directory = TempDir::new().expect("a temporary directory");
        let script = r#"echo '{"type":"system","subtype":"init","session_id":"s"}'
echo '{"type":"result","subtype":"success","is_error":false,"result":"One."}'
sleep 1
echo 'API Error: 429 Too Many Requests' >&2
sleep 1
echo '{"type":"system","subtype":"init","session_id":"s"}'
echo '{"type":"result","subtype":"success","is_error":false,"result":"Two."}'
sleep 1
exit 0"#;
        let provider = CliProvider::agent(
            AgentKind::Claude,
            settings(&directory, script)
                .with_tripwire(|line| line.contains("429"))
                .await_background(),
        );

        let execution = execute(&provider, &[Message::user("hi")]).await;

        assert!(
            execution
                .failure
                .as_deref()
                .is_some_and(|failure| failure.contains("429")),
            "{execution:?}"
        );
    }

    /// A result claude writes after the next turn began settles nothing while
    /// that turn is still being written.
    #[tokio::test]
    async fn a_result_written_after_the_next_turn_began_waits_for_that_turn() {
        let directory = TempDir::new().expect("a temporary directory");
        let script = r#"echo '{"type":"system","subtype":"init","session_id":"s"}'
echo '{"type":"system","subtype":"init","session_id":"s"}'
echo '{"type":"result","subtype":"success","is_error":false,"result":"First."}'
sleep 8
echo '{"type":"result","subtype":"success","is_error":false,"result":"Second."}'"#;
        let provider = CliProvider::agent(
            AgentKind::Claude,
            settings(&directory, script).await_background(),
        );

        let execution = execute(&provider, &[Message::user("hi")]).await;

        assert_eq!(execution.stopped, None, "{execution:?}");
        assert_eq!(
            execution.stdout.answer.as_deref(),
            Some("First.\n\nSecond.")
        );
        assert_ne!(
            execution.stdout.finish_reason.as_deref(),
            Some(crate::CUT_SHORT)
        );
    }

    /// A failure after a result that settled the run still fails it, and the
    /// turn that begins next reopens nothing.
    #[tokio::test]
    async fn a_failure_after_a_settling_result_fails_the_run_at_once() {
        let directory = TempDir::new().expect("a temporary directory");
        let started = directory.path().join("started");
        let script = format!(
            r#"touch '{started}'
echo '{{"type":"system","subtype":"init","session_id":"s"}}'
echo '{{"type":"result","subtype":"success","is_error":false,"result":"One."}}'
echo '{{"type":"result","subtype":"error_during_execution","is_error":true,"result":"Boom"}}'
echo '{{"type":"system","subtype":"init","session_id":"s"}}'
sleep 60"#,
            started = started.display(),
        );
        let provider = CliProvider::agent(
            AgentKind::Claude,
            settings(&directory, &script)
                .await_background()
                .with_timeout(Duration::from_secs(15)),
        );

        let (execution, waited) = timed(
            async { execute(&provider, &[Message::user("hi")]).await },
            &started,
        )
        .await;

        assert!(waited < Duration::from_secs(11), "waited {waited:?}");
        assert!(
            execution
                .failure
                .as_deref()
                .is_some_and(|failure| failure.contains("Boom")),
            "{execution:?}"
        );
    }

    /// An agent that says something on its way out when the timeout stops it.
    fn saying_on_its_way_out(directory: &TempDir, said: &str) -> CliSettings {
        let script = format!(
            r#"trap '{said}; exit 0' TERM
echo '{{"type":"system","subtype":"background_tasks_changed","tasks":[{{"task_id":"b1"}}]}}'
echo '{{"type":"result","subtype":"success","is_error":false,"result":"Kept."}}'
sleep 120 &
wait"#
        );
        settings(directory, &script)
            .await_background()
            .with_timeout(Duration::from_secs(6))
    }

    #[tokio::test]
    async fn a_trip_on_the_way_out_is_never_marked_cut_short() {
        let directory = TempDir::new().expect("a temporary directory");
        let settings =
            saying_on_its_way_out(&directory, "echo \"API Error: 429 Too Many Requests\" >&2")
                .with_tripwire(|line| line.contains("429"));
        let provider = CliProvider::agent(AgentKind::Claude, settings);

        let execution = execute(&provider, &[Message::user("hi")]).await;

        assert!(
            execution
                .failure
                .as_deref()
                .is_some_and(|failure| failure.contains("429")),
            "{execution:?}"
        );
        assert_ne!(
            execution.stdout.finish_reason.as_deref(),
            Some(crate::CUT_SHORT),
            "{execution:?}"
        );
    }

    #[tokio::test]
    async fn a_failure_reported_on_the_way_out_is_never_marked_cut_short() {
        let directory = TempDir::new().expect("a temporary directory");
        let provider = CliProvider::agent(
            AgentKind::Claude,
            saying_on_its_way_out(
                &directory,
                r#"echo "{\"type\":\"result\",\"subtype\":\"error_during_execution\",\"is_error\":true,\"result\":\"Stopped.\"}""#,
            ),
        );

        let execution = execute(&provider, &[Message::user("hi")]).await;

        assert!(execution.stdout.failure.is_some(), "{execution:?}");
        assert_ne!(
            execution.stdout.finish_reason.as_deref(),
            Some(crate::CUT_SHORT),
            "{execution:?}"
        );
    }

    /// An agent that closes its output with a later turn begun, and lives on.
    #[tokio::test]
    async fn output_closed_on_an_open_turn_settles_the_run() {
        let directory = TempDir::new().expect("a temporary directory");
        let started = directory.path().join("started");
        let script = format!(
            r#"touch '{started}'
echo '{{"type":"system","subtype":"init","session_id":"s"}}'
echo '{{"type":"result","subtype":"success","is_error":false,"result":"First."}}'
echo '{{"type":"system","subtype":"init","session_id":"s"}}'
exec 1>&-
sleep 60"#,
            started = started.display(),
        );
        let provider = CliProvider::agent(
            AgentKind::Claude,
            settings(&directory, &script)
                .await_background()
                .with_timeout(Duration::from_secs(20)),
        );

        let (execution, waited) = timed(
            async { execute(&provider, &[Message::user("hi")]).await },
            &started,
        )
        .await;

        assert!(waited < Duration::from_secs(14), "waited {waited:?}");
        assert_eq!(
            execution.stdout.finish_reason.as_deref(),
            Some(crate::CUT_SHORT),
            "{execution:?}"
        );
    }

    /// Far more than the answers an agent can write before the timeout.
    const OUTPUT_LIMIT_PAST_THE_TIMEOUT: usize = 512 * 1024 * 1024;

    /// Turns begun as fast as an agent can write them still end at the timeout.
    #[tokio::test]
    async fn turns_begun_faster_than_they_are_read_still_end_at_the_timeout() {
        let directory = TempDir::new().expect("a temporary directory");
        let started = directory.path().join("started");
        let script = format!(
            r#"touch '{started}'
while true; do
  echo '{{"type":"system","subtype":"init","session_id":"s"}}'
  echo '{{"type":"result","subtype":"success","is_error":false,"result":"a"}}'
done"#,
            started = started.display(),
        );
        let timeout = Duration::from_secs(6);
        let provider = CliProvider::agent(
            AgentKind::Claude,
            settings(&directory, &script)
                .await_background()
                .with_output_limit(OUTPUT_LIMIT_PAST_THE_TIMEOUT)
                .with_timeout(timeout),
        );

        let (outcome, waited) = timed(
            provider.execute(request(&[Message::user("hi")]), "test-run"),
            &started,
        )
        .await;

        let execution = outcome.expect("the answers it gave, not the answer limit");
        assert!(
            waited < timeout + Duration::from_secs(6),
            "waited {waited:?}"
        );
        assert!(
            execution
                .stopped
                .as_deref()
                .is_some_and(|reason| reason.contains("timeout")),
            "stopped by the deadline: {:?}",
            execution.stopped
        );
        assert_eq!(
            execution.stdout.finish_reason.as_deref(),
            Some(crate::CUT_SHORT)
        );
    }

    /// Answers joined across turns are held to the output limit, as prose is.
    #[tokio::test]
    async fn answers_joined_across_turns_are_held_to_the_output_limit() {
        let directory = TempDir::new().expect("a temporary directory");
        let answer = "y".repeat(600);
        let script = format!(
            r#"echo '{{"type":"system","subtype":"init","session_id":"s"}}'
echo '{{"type":"result","subtype":"success","is_error":false,"result":"{answer}"}}'
echo '{{"type":"system","subtype":"init","session_id":"s"}}'
echo '{{"type":"result","subtype":"success","is_error":false,"result":"{answer}"}}'"#
        );
        let provider = CliProvider::agent(
            AgentKind::Claude,
            settings(&directory, &script)
                .await_background()
                .with_output_limit(1024),
        );

        let outcome = provider
            .execute(request(&[Message::user("hi")]), "test-run")
            .await;

        let failure = outcome.expect_err("answers past the output limit");
        assert!(
            failure.to_string().contains(crate::PROSE_EXCEEDED),
            "{failure:?}"
        );
    }

    /// A turn that said nothing has not answered, so the timeout still fails
    /// the run.
    #[tokio::test]
    async fn an_empty_turn_outliving_the_timeout_fails_the_run() {
        let directory = TempDir::new().expect("a temporary directory");
        let script = r#"echo '{"type":"system","subtype":"background_tasks_changed","tasks":[{"task_id":"b1"}]}'
echo '{"type":"result","subtype":"success","is_error":false,"result":""}'
sleep 120"#;
        let provider = CliProvider::agent(
            AgentKind::Claude,
            settings(&directory, script)
                .await_background()
                .with_timeout(Duration::from_secs(6)),
        );

        let failure = provider
            .execute(request(&[Message::user("hi")]), "test-run")
            .await
            .expect_err("a run that never answered");

        assert!(
            matches!(*failure.error, ProviderError::Timeout { .. }),
            "{failure:?}"
        );
    }

    /// An `init` too long to read still begins a turn.
    #[tokio::test]
    async fn a_turn_whose_init_is_too_long_to_read_still_counts_as_begun() {
        let directory = TempDir::new().expect("a temporary directory");
        let tools = "t".repeat(8192);
        let script = format!(
            r#"echo '{{"type":"system","subtype":"init","session_id":"s","tools":["{tools}"]}}'
echo '{{"type":"result","subtype":"success","is_error":false,"result":"First."}}'
echo '{{"type":"system","subtype":"init","session_id":"s","tools":["{tools}"]}}'
echo '{{"type":"assistant","message":{{"content":[{{"type":"text","text":"The la"}}]}}}}'"#
        );
        let provider = CliProvider::agent(
            AgentKind::Claude,
            settings(&directory, &script).with_line_limit(4096),
        );

        let execution = execute(&provider, &[Message::user("hi")]).await;

        assert_eq!(
            execution.stdout.finish_reason.as_deref(),
            Some(crate::CUT_SHORT),
            "{execution:?}"
        );
    }

    /// A trip on stderr with a later turn open is the run's failure, never a
    /// turn cut short.
    #[tokio::test]
    async fn a_trip_with_a_later_turn_open_is_never_marked_cut_short() {
        let directory = TempDir::new().expect("a temporary directory");
        let script = r#"echo '{"type":"system","subtype":"init","session_id":"s"}'
echo '{"type":"result","subtype":"success","is_error":false,"result":"First."}'
echo '{"type":"system","subtype":"init","session_id":"s"}'
echo 'API Error: 429 Too Many Requests' >&2
exit 0"#;
        let provider = CliProvider::agent(
            AgentKind::Claude,
            settings(&directory, script).with_tripwire(|line| line.contains("429")),
        );

        let execution = execute(&provider, &[Message::user("hi")]).await;

        assert!(
            execution
                .failure
                .as_deref()
                .is_some_and(|failure| failure.contains("429")),
            "{execution:?}"
        );
        assert_ne!(
            execution.stdout.finish_reason.as_deref(),
            Some(crate::CUT_SHORT),
            "{execution:?}"
        );
    }

    /// A later turn that fails is a failure, never a turn cut short.
    #[tokio::test]
    async fn a_later_turn_that_fails_is_never_marked_cut_short() {
        let directory = TempDir::new().expect("a temporary directory");
        let script = r#"echo '{"type":"system","subtype":"init","session_id":"s"}'
echo '{"type":"result","subtype":"success","is_error":false,"result":"First."}'
echo '{"type":"system","subtype":"init","session_id":"s"}'
echo '{"type":"result","subtype":"error_during_execution","is_error":true,"result":"Boom"}'"#;
        let provider = CliProvider::agent(AgentKind::Claude, settings(&directory, script));

        let execution = execute(&provider, &[Message::user("hi")]).await;

        assert!(
            execution
                .stdout
                .failure
                .as_deref()
                .is_some_and(|failure| failure.contains("Boom")),
            "{execution:?}"
        );
        assert_ne!(
            execution.stdout.finish_reason.as_deref(),
            Some(crate::CUT_SHORT),
            "{execution:?}"
        );
    }

    /// A failure the agent reports as the timeout stops it is the run's
    /// failure, not a bare timeout.
    #[tokio::test]
    async fn a_failure_reported_as_the_timeout_stops_an_unanswered_run_is_kept() {
        let directory = TempDir::new().expect("a temporary directory");
        let script = r#"trap 'echo "{\"type\":\"result\",\"subtype\":\"error_during_execution\",\"is_error\":true,\"result\":\"Stopped.\"}"; exit 0' TERM
echo '{"type":"system","subtype":"background_tasks_changed","tasks":[{"task_id":"b1"}]}'
sleep 120 &
wait"#;
        let provider = CliProvider::agent(
            AgentKind::Claude,
            settings(&directory, script)
                .await_background()
                .with_timeout(Duration::from_secs(6)),
        );

        let error = run(&provider, &[Message::user("hi")])
            .await
            .expect_err("the failure it reported");

        assert!(
            matches!(&error, ProviderError::Agent { message, .. } if message.contains("Stopped.")),
            "{error:?}"
        );
    }

    /// A schema-shaped answer is an answer, so the timeout keeps it.
    #[tokio::test]
    async fn a_structured_answer_outliving_the_timeout_is_kept() {
        let directory = TempDir::new().expect("a temporary directory");
        let script = r#"echo '{"type":"system","subtype":"background_tasks_changed","tasks":[{"task_id":"b1"}]}'
echo '{"type":"result","subtype":"success","is_error":false,"result":"","structured_output":{"verdict":"sound"}}'
sleep 120"#;
        let provider = CliProvider::agent(
            AgentKind::Claude,
            settings(&directory, script)
                .await_background()
                .with_timeout(Duration::from_secs(6)),
        );

        let execution = provider
            .execute(request(&[Message::user("hi")]), "test-run")
            .await
            .expect("the structured answer it gave");

        assert_eq!(
            execution.stdout.structured,
            Some(serde_json::json!({"verdict": "sound"}))
        );
        assert_eq!(
            execution.stdout.finish_reason.as_deref(),
            Some(crate::CUT_SHORT)
        );
    }

    #[test]
    fn only_claude_waits_on_background_work() {
        let directory = TempDir::new().expect("a temporary directory");
        let awaiting = |agent| {
            CliProvider::agent(agent, settings(&directory, "exit 0").await_background())
                .awaits_background()
        };

        assert!(awaiting(AgentKind::Claude));
        assert!(
            !awaiting(AgentKind::Codex),
            "codex reports no background work"
        );
    }

    /// A run that waits on background work and is tripped while it waits has
    /// failed, though a turn finished before: later work went wrong.
    #[tokio::test]
    async fn a_trip_while_waiting_on_background_work_fails_the_run() {
        let directory = TempDir::new().expect("a temporary directory");
        let script = r#"echo '{"type":"system","subtype":"background_tasks_changed","tasks":[{"task_id":"b1"}]}'
echo '{"type":"result","subtype":"success","is_error":false,"result":"Done."}'
sleep 1
echo 'API Error: 429 Too Many Requests' >&2
sleep 2
exit 0"#;
        let provider = CliProvider::agent(
            AgentKind::Claude,
            settings(&directory, script)
                .with_tripwire(|line| line.contains("429"))
                .await_background(),
        );

        let error = run(&provider, &[Message::user("hi")])
            .await
            .expect_err("the trip fails the run");
        let (_, streamed_failure, _) =
            collected(streamed(&provider, &[Message::user("hi")]).await).await;

        assert!(error.to_string().contains("429"), "{error:?}");
        assert!(
            streamed_failure.is_some_and(|failure| failure.to_string().contains("429")),
            "the stream ends in the same failure"
        );
    }

    /// Without the setting, a trip after a finished turn leaves its answer
    /// standing, as it always has for every caller.
    #[tokio::test]
    async fn a_trip_after_a_finished_turn_leaves_the_answer_without_the_setting() {
        let directory = TempDir::new().expect("a temporary directory");
        let script = r#"echo '{"type":"result","subtype":"success","is_error":false,"result":"Done."}'
sleep 1
echo 'API Error: 429 Too Many Requests' >&2
sleep 1
exit 0"#;
        let provider = CliProvider::agent(
            AgentKind::Claude,
            settings(&directory, script).with_tripwire(|line| line.contains("429")),
        );

        let completion = run(&provider, &[Message::user("hi")])
            .await
            .expect("the answer stands");

        assert_eq!(completion.message.content.as_deref(), Some("Done."));
    }

    /// A result, a turn and its result read in one go settle the run twice with
    /// a reopening between, none of which may be lost.
    #[tokio::test]
    async fn turns_read_in_one_go_settle_the_run_without_waiting_for_its_timeout() {
        let directory = TempDir::new().expect("a temporary directory");
        let started = directory.path().join("started");
        let turns = directory.path().join("turns.jsonl");
        std::fs::write(
            &turns,
            [
                r#"{"type":"system","subtype":"init","session_id":"s"}"#,
                r#"{"type":"result","subtype":"success","is_error":false,"result":"One."}"#,
                r#"{"type":"system","subtype":"init","session_id":"s"}"#,
                r#"{"type":"result","subtype":"success","is_error":false,"result":"Two."}"#,
            ]
            .join("\n")
                + "\n",
        )
        .expect("the turns");
        let script = format!(
            "touch '{started}'\ncat '{turns}'\nsleep 120",
            started = started.display(),
            turns = turns.display(),
        );
        let timeout = Duration::from_secs(14);
        let provider = CliProvider::agent(
            AgentKind::Claude,
            settings(&directory, &script)
                .await_background()
                .with_timeout(timeout),
        );

        let (execution, waited) = timed(
            async { execute(&provider, &[Message::user("hi")]).await },
            &started,
        )
        .await;

        assert!(waited < Duration::from_secs(11), "waited {waited:?}");
        assert_ne!(
            execution.stdout.finish_reason.as_deref(),
            Some(crate::CUT_SHORT)
        );
    }

    /// Claude can begin a turn before writing the result of the one before, so
    /// a turn stays open while more have begun than finished.
    #[tokio::test]
    async fn a_turn_begun_before_the_last_result_is_still_open_when_output_ends() {
        let directory = TempDir::new().expect("a temporary directory");
        let script = r#"echo '{"type":"system","subtype":"init","session_id":"s"}'
echo '{"type":"system","subtype":"init","session_id":"s"}'
echo '{"type":"assistant","message":{"content":[{"type":"text","text":"The la"}]}}'
echo '{"type":"result","subtype":"success","is_error":false,"result":"First."}'"#;
        let provider = CliProvider::agent(AgentKind::Claude, settings(&directory, script));

        let execution = execute(&provider, &[Message::user("hi")]).await;

        assert_eq!(
            execution.stdout.finish_reason.as_deref(),
            Some(crate::CUT_SHORT),
            "{execution:?}"
        );
    }

    #[tokio::test]
    async fn a_later_turn_that_fails_its_stream_is_never_marked_cut_short_first() {
        let directory = TempDir::new().expect("a temporary directory");
        let long = "x".repeat(4096);
        let script = format!(
            r#"echo '{{"type":"system","subtype":"init","session_id":"s"}}'
echo '{{"type":"result","subtype":"success","is_error":false,"result":"First."}}'
echo '{{"type":"system","subtype":"init","session_id":"s"}}'
echo '{{"type":"assistant","message":{{"content":[{{"type":"text","text":"{long}"}}]}}}}'"#
        );
        let provider = CliProvider::agent(
            AgentKind::Claude,
            settings(&directory, &script).with_output_limit(1024),
        );

        let events = streamed(&provider, &[Message::user("hi")]).await;
        let (events, failure, _) = collected(events).await;

        assert!(failure.is_some(), "{events:?}");
        assert!(
            !events.iter().any(|event| matches!(
                event,
                AgentEvent::Finished { finish_reason }
                    if finish_reason.as_deref() == Some(crate::CUT_SHORT)
            )),
            "{events:?}"
        );
    }

    #[tokio::test]
    async fn a_run_over_two_results_counts_them_both() {
        let directory = TempDir::new().expect("a temporary directory");
        let provider = CliProvider::agent(
            AgentKind::Claude,
            replaying_turns(&directory, BACKGROUND_COMMAND).await_background(),
        );

        let execution = execute(&provider, &[Message::user("hi")]).await;

        let stdout = &execution.stdout;
        let usage = stdout.usage.as_ref().expect("the run's usage");
        assert_eq!(
            (usage.prompt_tokens, usage.completion_tokens),
            (30, 15),
            "the sum claude's own modelUsage reports"
        );
        let tokens = stdout.tokens.as_ref().expect("the run's tokens");
        assert_eq!(
            (tokens.prompt_tokens(), tokens.output_tokens),
            (30, Some(15))
        );
        assert_eq!(stdout.turns, Some(3), "{stdout:?}");
    }

    #[tokio::test]
    async fn a_completion_over_two_results_keeps_both_turns_words() {
        let directory = TempDir::new().expect("a temporary directory");
        let provider = CliProvider::agent(
            AgentKind::Claude,
            replaying_turns(&directory, BACKGROUND_COMMAND).await_background(),
        );

        let completion = run(&provider, &[Message::user("hi")])
            .await
            .expect("an answer");

        let content = completion.message.content.expect("the answer's words");
        assert!(content.contains(FIRST_ANSWER), "{content}");
        assert!(content.contains(LATER_ANSWER), "{content}");
    }

    #[tokio::test]
    async fn a_stream_awaiting_background_work_delivers_the_later_turn_without_an_error() {
        let directory = TempDir::new().expect("a temporary directory");
        let provider = CliProvider::agent(
            AgentKind::Claude,
            replaying_turns(&directory, BACKGROUND_COMMAND).await_background(),
        );

        let events = streamed(&provider, &[Message::user("hi")]).await;
        let (events, failure, _) = collected(events).await;

        assert!(failure.is_none(), "{failure:?}");
        let finishes = events
            .iter()
            .filter(|event| matches!(event, AgentEvent::Finished { .. }))
            .count();
        assert_eq!(finishes, 2, "{events:?}");
        assert!(
            events.iter().any(
                |event| matches!(event, AgentEvent::Text(text) if text.contains(LATER_ANSWER))
            ),
            "{events:?}"
        );
    }

    /// A self-pacing loop or a server left running never lets the run settle,
    /// but it has answered: the timeout costs only what it was still waiting
    /// for, and says so.
    #[tokio::test]
    async fn background_work_outliving_the_timeout_keeps_the_answer_marked_cut_short() {
        let directory = TempDir::new().expect("a temporary directory");
        let provider = CliProvider::agent(AgentKind::Claude, outliving_its_timeout(&directory));

        let execution = execute(&provider, &[Message::user("hi")]).await;
        let completion = run(&provider, &[Message::user("hi")])
            .await
            .expect("the answer it gave");

        assert!(execution.stopped.is_some(), "{execution:?}");
        assert_eq!(execution.failure, None, "{execution:?}");
        assert_eq!(completion.message.content.as_deref(), Some("Kept."));
        assert_eq!(completion.finish_reason.as_deref(), Some(crate::CUT_SHORT));
    }

    #[tokio::test]
    async fn a_stream_outliving_its_timeout_after_answering_ends_marked_cut_short() {
        let directory = TempDir::new().expect("a temporary directory");
        let provider = CliProvider::agent(AgentKind::Claude, outliving_its_timeout(&directory));

        let events = streamed(&provider, &[Message::user("hi")]).await;
        let (events, failure, _) = collected(events).await;

        assert!(failure.is_none(), "{failure:?}");
        assert!(
            matches!(
                events.last(),
                Some(AgentEvent::Finished { finish_reason }) if finish_reason.as_deref() == Some(crate::CUT_SHORT)
            ),
            "{events:?}"
        );
    }

    fn outliving_its_timeout(directory: &TempDir) -> CliSettings {
        let script = r#"echo '{"type":"system","subtype":"background_tasks_changed","tasks":[{"task_id":"b1"}]}'
echo '{"type":"assistant","message":{"content":[{"type":"text","text":"Kept."}]}}'
echo '{"type":"result","subtype":"success","is_error":false,"result":"Kept."}'
sleep 120"#;
        settings(directory, script)
            .await_background()
            .with_timeout(Duration::from_secs(6))
    }

    /// A `Monitor` line starts a turn of its own with nothing listed to owe it,
    /// after a result that settled the run.
    const UNOWED_TURN: &str = r#"echo '{"type":"system","subtype":"init","session_id":"s"}'
echo '{"type":"assistant","message":{"content":[{"type":"text","text":"Watching."}]}}'
echo '{"type":"result","subtype":"success","is_error":false,"result":"Watching."}'
echo '{"type":"system","subtype":"init","session_id":"s"}'
sleep 8
echo '{"type":"assistant","message":{"content":[{"type":"text","text":"It ticked."}]}}'
echo '{"type":"result","subtype":"success","is_error":false,"result":"It ticked."}'"#;

    #[tokio::test]
    async fn a_turn_that_begins_after_the_run_settled_reopens_the_wait() {
        let directory = TempDir::new().expect("a temporary directory");
        let provider = CliProvider::agent(
            AgentKind::Claude,
            settings(&directory, UNOWED_TURN).await_background(),
        );

        let completion = run(&provider, &[Message::user("hi")])
            .await
            .expect("both turns");

        assert_eq!(
            completion.message.content.as_deref(),
            Some("Watching.\n\nIt ticked.")
        );
        assert_ne!(completion.finish_reason.as_deref(), Some(crate::CUT_SHORT));
    }

    /// Without the setting the cutoff stands, and the turn it cut off says so.
    #[tokio::test]
    async fn a_run_not_awaiting_background_work_marks_the_turn_its_cutoff_ended() {
        let directory = TempDir::new().expect("a temporary directory");
        let provider = CliProvider::agent(AgentKind::Claude, settings(&directory, UNOWED_TURN));

        let execution = execute(&provider, &[Message::user("hi")]).await;
        let completion = assemble("claude", execution).expect("the first turn's answer");

        assert_eq!(completion.message.content.as_deref(), Some("Watching."));
        assert_eq!(completion.finish_reason.as_deref(), Some(crate::CUT_SHORT));
    }

    /// Each turn that begins in the grace reopens the wait, and the run's
    /// timeout still ends it.
    #[tokio::test]
    async fn an_agent_that_keeps_beginning_turns_is_bounded_by_the_timeout() {
        let directory = TempDir::new().expect("a temporary directory");
        let started = directory.path().join("started");
        let script = format!(
            r#"touch '{started}'
echo '{{"type":"result","subtype":"success","is_error":false,"result":"Again."}}'
while true; do
  sleep 2
  echo '{{"type":"system","subtype":"init","session_id":"s"}}'
  echo '{{"type":"result","subtype":"success","is_error":false,"result":"Again."}}'
done"#,
            started = started.display(),
        );
        let timeout = Duration::from_secs(10);
        let provider = CliProvider::agent(
            AgentKind::Claude,
            settings(&directory, &script)
                .await_background()
                .with_timeout(timeout),
        );

        let (completion, waited) = timed(run(&provider, &[Message::user("hi")]), &started).await;

        let completion = completion.expect("the answers it gave");
        assert!(
            waited > timeout - Duration::from_secs(3),
            "stopped at {waited:?}, before reopening ran it to its timeout"
        );
        assert!(
            waited < timeout + Duration::from_secs(5),
            "waited {waited:?}"
        );
        assert!(
            completion
                .message
                .content
                .as_deref()
                .is_some_and(|content| content.matches("Again.").count() > 3),
            "{completion:?}"
        );
    }

    #[tokio::test]
    async fn a_later_turn_the_agent_exits_part_way_through_keeps_the_earlier_answer_marked() {
        let directory = TempDir::new().expect("a temporary directory");
        let script = r#"echo '{"type":"system","subtype":"init","session_id":"s"}'
echo '{"type":"system","subtype":"background_tasks_changed","tasks":[{"task_id":"b1"}]}'
echo '{"type":"result","subtype":"success","is_error":false,"result":"Started."}'
echo '{"type":"system","subtype":"task_notification","task_id":"b1","status":"completed"}'
echo '{"type":"system","subtype":"background_tasks_changed","tasks":[]}'
echo '{"type":"system","subtype":"init","session_id":"s"}'
echo '{"type":"assistant","message":{"content":[{"type":"text","text":"The build fai"}]}}'"#;
        let provider = CliProvider::agent(
            AgentKind::Claude,
            settings(&directory, script).await_background(),
        );

        let completion = run(&provider, &[Message::user("hi")])
            .await
            .expect("the earlier turn's answer");

        assert_eq!(completion.message.content.as_deref(), Some("Started."));
        assert_eq!(completion.finish_reason.as_deref(), Some(crate::CUT_SHORT));
    }

    /// Nothing has answered yet, so running out of time is still a failure.
    #[tokio::test]
    async fn background_work_outliving_the_timeout_before_any_answer_fails_the_run() {
        let directory = TempDir::new().expect("a temporary directory");
        let script = r#"echo '{"type":"system","subtype":"background_tasks_changed","tasks":[{"task_id":"b1"}]}'
sleep 120"#;
        let settings = settings(&directory, script)
            .await_background()
            .with_timeout(Duration::from_secs(8));
        let provider = CliProvider::agent(AgentKind::Claude, settings);

        let failure = provider
            .execute(request(&[Message::user("hi")]), "test-run")
            .await
            .expect_err("a run past its timeout");

        assert!(
            matches!(*failure.error, ProviderError::Timeout { .. }),
            "{failure:?}"
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
        let error = assemble(provider.name(), execution).expect_err("a failure");
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
            let error = assemble(provider.name(), execution).expect_err("a failure");
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
        let error = assemble(provider.name(), execution).expect_err("a failure");
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

        let error = assemble(provider.name(), execution).expect_err("a failure");
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
echo >&2
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
        let error = assemble(provider.name(), execution).expect_err("a failure");

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
        let completion = assemble(provider.name(), execution).expect("an answer");
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

    #[tokio::test]
    async fn the_session_a_turn_is_given_reaches_the_agents_command_line() {
        let directory = TempDir::new().expect("a temporary directory");
        let captured = directory.path().join("arguments");
        let script = format!(
            r#"printf '%s\n' "$@" > '{captured}'
echo '{{"type":"result","subtype":"success","is_error":false}}'"#,
            captured = captured.display(),
        );
        let arguments = |settings: CliSettings| {
            let captured = captured.clone();
            async move {
                run(
                    &CliProvider::agent(AgentKind::Claude, settings),
                    &[Message::user("hi")],
                )
                .await
                .expect("an answer");
                std::fs::read_to_string(&captured)
                    .expect("the captured arguments")
                    .lines()
                    .map(str::to_string)
                    .collect::<Vec<String>>()
            }
        };

        let resumed = arguments(
            settings(&directory, &script)
                .with_session(Session::resumed("5b0c1f7e-8d43-4a77-9a3e-2f0d6c1b9e42")),
        )
        .await;
        assert_eq!(
            &resumed[resumed.len() - 3..],
            [
                "--resume",
                "5b0c1f7e-8d43-4a77-9a3e-2f0d6c1b9e42",
                "--print"
            ],
            "{resumed:?}"
        );

        let unpinned = arguments(settings(&directory, &script)).await;
        assert!(
            !unpinned
                .iter()
                .any(|argument| argument == "--resume" || argument == "--session-id"),
            "a turn given no session was pinned to one: {unpinned:?}"
        );
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

    /// A required server Codex cannot be given fails the run before the agent
    /// starts, rather than the run going ahead without it.
    #[tokio::test]
    async fn a_codex_run_fails_when_a_required_server_cannot_attach() {
        let directory = TempDir::new().expect("a temporary directory");
        let captured = directory.path().join("arguments");
        let recorded = directory.path().join("environment");
        let settings = settings(&directory, &recording_codex(&captured, &recorded))
            .with_mcp_server(
                "events",
                McpServer::remote("https://mcp.example.com/sse")
                    .with_transport(McpTransport::Sse)
                    .require(),
            );
        let provider = CliProvider::agent(AgentKind::Codex, settings);

        let error = run(&provider, &[Message::user("hi")])
            .await
            .expect_err("a required server that cannot attach fails the run");

        assert!(error.to_string().contains("events"), "{error}");
        assert!(!captured.exists(), "the agent started anyway");
    }

    /// Claude Code has no required servers, so a Claude run goes ahead
    /// without one it cannot be given, as it always has.
    #[tokio::test]
    async fn a_claude_run_ignores_a_required_server_it_cannot_attach() {
        let directory = TempDir::new().expect("a temporary directory");
        let answer =
            r#"echo '{"type":"result","subtype":"success","is_error":false,"result":"ok"}'"#;
        let settings = settings(&directory, answer).with_mcp_server(
            "bad name",
            McpServer::remote("https://mcp.example.com/mcp").require(),
        );
        let provider = CliProvider::agent(AgentKind::Claude, settings);

        run(&provider, &[Message::user("hi")])
            .await
            .expect("an answer");
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
            "Looking now. \n\nIt is empty."
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
        let error = assemble(provider.name(), execution).expect_err("a failure");
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
            assert!(
                matches!(&error, ProviderError::Limited { limit, .. } if limit.credits && limit.message == failure),
                "{error:?}"
            );
        }
    }

    const SESSION_LIMIT: &str = "You've hit your session limit \u{b7} resets 5pm";

    /// claude refusing the main agent's request past the plan's five-hour
    /// window, after it had begun to answer.
    fn refused_after_text() -> [String; 4] {
        [
            json!({"type": "assistant", "message": {"content": [{"type": "text", "text": "Halfway there."}]}, "parent_tool_use_id": null}),
            json!({"type": "rate_limit_event", "rate_limit_info": {"status": "rejected", "resetsAt": 1_790_208_000, "rateLimitType": "five_hour", "isUsingOverage": false}}),
            json!({"type": "assistant", "message": {"model": "<synthetic>", "content": [{"type": "text", "text": SESSION_LIMIT}]}, "parent_tool_use_id": null, "error": "rate_limit", "is_api_error_message": true}),
            json!({"type": "result", "subtype": "success", "is_error": true, "api_error_status": 429, "result": SESSION_LIMIT}),
        ]
        .map(|line| line.to_string())
    }

    #[tokio::test]
    async fn a_limited_turn_ends_the_stream_as_a_limit_error() {
        let directory = TempDir::new().expect("a temporary directory");
        let provider = CliProvider::agent(
            AgentKind::Claude,
            settings(&directory, &replaying(&directory, &refused_after_text())),
        );

        let (delivered, failure, after) =
            collected(streamed(&provider, &[Message::user("Carry on.")]).await).await;

        assert!(
            matches!(
                delivered.as_slice(),
                [AgentEvent::Text(text), AgentEvent::Diagnostic(remembered)]
                    if text == "Halfway there."
                        && remembered == "rate limit reached (five_hour, rejected)"
            ),
            "{delivered:?}"
        );
        assert_eq!(after, 0, "the stream went on past the limit");
        let failure = failure.expect("a limited turn ends in an error");
        let ProviderError::Limited {
            provider, limit, ..
        } = &failure
        else {
            panic!("expected a limit, got {failure:?}");
        };
        assert_eq!(provider, "claude");
        assert_eq!(
            limit.message,
            format!("rate limit reached (five_hour, rejected): {SESSION_LIMIT}")
        );
        assert_eq!(
            limit.resets_at,
            chrono::DateTime::from_timestamp(1_790_208_000, 0)
        );
        assert!(!limit.credits);
        assert_eq!(
            limit.window.as_ref().map(|window| window.name.as_str()),
            Some("five_hour")
        );
        assert_eq!(
            failure.to_string(),
            format!("claude: rate limit reached (five_hour, rejected): {SESSION_LIMIT}")
        );
    }

    #[tokio::test]
    async fn a_limited_turn_is_the_same_limit_streamed_completed_or_executed() {
        let directory = TempDir::new().expect("a temporary directory");
        let script = format!(
            "echo 'warning: the plan is spent' >&2\n{}",
            replaying(&directory, &refused_after_text())
        );
        let provider = CliProvider::agent(AgentKind::Claude, settings(&directory, &script));
        let messages = [Message::user("Carry on.")];

        let (_, streamed, _) = collected(streamed(&provider, &messages).await).await;
        let completed = run(&provider, &messages).await.expect_err("a limited turn");
        let execution = execute(&provider, &messages).await;
        let words = execution
            .stdout
            .limit
            .clone()
            .expect("the execution's limit");
        let executed = assemble(provider.name(), execution).expect_err("a limited turn");

        let limits = [streamed.expect("a limited stream"), completed, executed].map(|error| {
            let ProviderError::Limited { limit, .. } = error else {
                panic!("expected a limit, got {error:?}");
            };
            *limit
        });
        assert_eq!(limits[0], limits[1]);
        assert_eq!(limits[1], limits[2]);
        let tail = format!("{STDERR_HEADING}warning: the plan is spent");
        assert!(limits[0].message.ends_with(&tail), "{:?}", limits[0]);
        assert_eq!(
            words.message,
            format!("rate limit reached (five_hour, rejected): {SESSION_LIMIT}")
        );
        assert_eq!(
            limits[0].message,
            format!("{}{tail}", words.message),
            "the execution's limit is the words alone"
        );
        assert_eq!(limits[0].clone().with_message(words.message.clone()), words);
    }

    #[tokio::test]
    async fn a_stream_that_ends_on_a_refused_window_fails_in_its_words_and_not_as_a_limit() {
        let directory = TempDir::new().expect("a temporary directory");
        let script = r#"echo '{"type":"rate_limit_event","rate_limit_info":{"status":"rejected","rateLimitType":"five_hour"}}'"#;
        let provider = CliProvider::agent(AgentKind::Claude, settings(&directory, script));
        let messages = [Message::user("hi")];

        let (_, streamed, _) = collected(streamed(&provider, &messages).await).await;
        let completed = run(&provider, &messages).await.expect_err("a failed turn");

        for error in [streamed.expect("a failed stream"), completed] {
            let ProviderError::Agent { message, .. } = &error else {
                panic!("expected the agent's failure, got {error:?}");
            };
            assert!(
                message.starts_with("rate limit reached (five_hour, rejected)"),
                "{message}"
            );
        }
    }

    #[tokio::test]
    async fn a_limit_is_scrubbed_of_the_runs_secrets_everywhere() {
        for (agent, script) in [
            (
                AgentKind::Claude,
                r#"printf '{"type":"result","is_error":true,"api_error_status":429,"result":"You'"'"'ve hit your limit for %s"}
' "$DB_PASSWORD""#,
            ),
            (
                AgentKind::Codex,
                r#"printf '{"type":"turn.failed","error":{"message":"You have hit your usage limit for %s"}}
' "$DB_PASSWORD""#,
            ),
        ] {
            let directory = TempDir::new().expect("a temporary directory");
            let root = directory.path().join("logs");
            let settings = settings(&directory, script)
                .with_log(&root)
                .with_credential(Credential::key("DB_PASSWORD", "correct-horse-battery"));
            let provider = CliProvider::agent(agent, settings);

            let execution = execute(&provider, &[Message::user("hi")]).await;

            let limit = execution.stdout.limit.clone().expect("a limit");
            assert!(limit.message.ends_with("[REDACTED]"), "{agent}: {limit:?}");
            assert_eq!(
                execution.stdout.failure.as_deref(),
                Some(limit.message.as_str()),
                "{agent}"
            );
            for text in [
                &limit.message,
                execution.stdout.failure.as_ref().expect("a failure"),
            ] {
                assert!(!text.contains("correct-horse-battery"), "{agent}: {text}");
            }
            let files = execution.log.clone().expect("log files");
            for path in [&files.stdout, &files.stderr, &files.events] {
                let contents = std::fs::read_to_string(path).expect("a log file");
                assert!(
                    !contents.contains("correct-horse-battery"),
                    "{agent}: {} leaked the secret",
                    path.display()
                );
            }
            let error = assemble(provider.name(), execution).expect_err("a limited turn");
            let ProviderError::Limited { limit, .. } = &error else {
                panic!("{agent}: expected a limit, got {error:?}");
            };
            assert!(!limit.message.contains("correct-horse-battery"), "{agent}");
            assert!(
                !error.to_string().contains("correct-horse-battery"),
                "{agent}"
            );
        }
    }

    #[tokio::test]
    async fn a_window_report_reaches_the_consumer_without_ending_the_turn() {
        let stream = [
            json!({"type": "rate_limit_event", "rate_limit_info": {"status": "allowed", "resetsAt": 1_790_208_000, "rateLimitType": "five_hour", "utilization": 0.43, "isUsingOverage": false}}),
            json!({"type": "assistant", "message": {"content": [{"type": "text", "text": "Done."}]}, "parent_tool_use_id": null}),
            json!({"type": "result", "subtype": "success", "is_error": false, "result": "Done."}),
        ]
        .map(|line| line.to_string());
        let directory = TempDir::new().expect("a temporary directory");
        let provider = CliProvider::agent(
            AgentKind::Claude,
            settings(&directory, &replaying(&directory, &stream)),
        );
        let messages = [Message::user("Carry on.")];

        let (delivered, failure, _) = collected(streamed(&provider, &messages).await).await;

        assert!(failure.is_none(), "{failure:?}");
        let [
            AgentEvent::Window(window),
            AgentEvent::Text(text),
            AgentEvent::Answer(answer),
            AgentEvent::Finished { .. },
        ] = delivered.as_slice()
        else {
            panic!("expected the window, then the answer: {delivered:?}");
        };
        assert_eq!(window.name, "five_hour");
        assert_eq!(window.kind, Some(WindowKind::FiveHours));
        assert_eq!(window.used_percent, Some(43.0));
        assert_eq!(text, "Done.");
        assert_eq!(answer, "Done.");

        let completion = run(&provider, &messages)
            .await
            .expect("a window is no failure");
        assert_eq!(completion.message.content.as_deref(), Some("Done."));
        let execution = execute(&provider, &messages).await;
        assert_eq!(execution.stdout.windows, std::slice::from_ref(window));
    }

    #[tokio::test]
    async fn a_limit_carries_the_end_of_stderr_after_the_agents_words() {
        let directory = TempDir::new().expect("a temporary directory");
        let script = format!(
            "echo 'warning: the plan is spent' >&2\n{}",
            replaying(&directory, &refused_after_text())
        );
        let provider = CliProvider::agent(AgentKind::Claude, settings(&directory, &script));

        let error = run(&provider, &[Message::user("Carry on.")])
            .await
            .expect_err("a limited turn");

        let ProviderError::Limited { limit, .. } = &error else {
            panic!("expected a limit, got {error:?}");
        };
        assert_eq!(
            limit.message,
            format!(
                "rate limit reached (five_hour, rejected): {SESSION_LIMIT}{STDERR_HEADING}warning: the plan is spent"
            )
        );
        assert_eq!(error.to_string(), format!("claude: {}", limit.message));
    }

    #[tokio::test]
    async fn a_codex_turn_refused_for_its_usage_limit_ends_in_a_limit() {
        let directory = TempDir::new().expect("a temporary directory");
        let script = r#"
echo '{"type":"thread.started","thread_id":"019b2c41-0000-7000-8000-000000000002"}'
echo '{"type":"error","message":"You have hit your usage limit. Try again later."}'
echo '{"type":"turn.failed","error":{"message":"You have hit your usage limit. Try again later."}}'
"#;
        let provider = CliProvider::agent(AgentKind::Codex, settings(&directory, script));

        let error = run(&provider, &[Message::user("hi")])
            .await
            .expect_err("a limited turn");

        let ProviderError::Limited {
            provider, limit, ..
        } = &error
        else {
            panic!("expected a limit, got {error:?}");
        };
        assert_eq!(provider, "codex");
        assert_eq!(
            limit.message,
            "You have hit your usage limit. Try again later."
        );
        assert_eq!(limit.resets_at, None);
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
    /// the window and the turn's working directory, and never with its
    /// credential.
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
                .with_working_directory(directory.path())
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
        assert!(
            credited[0].contains(&format!("directory={}", directory.path().display())),
            "{logged}"
        );
        assert_eq!(
            logged
                .lines()
                .filter(|line| line.contains("INFO") && line.contains("abnegate_agent_cli::reader"))
                .count(),
            1,
            "{logged}"
        );
        assert!(!logged.contains(TOKEN), "{logged}");
    }

    /// The working directory a credits line names is scrubbed of the run's
    /// secrets like everything else the run logs.
    #[test]
    fn a_credits_line_scrubs_the_secrets_in_its_directory() {
        const TOKEN: &str = concat!(
            "sk-ant-",
            "api03-",
            "CCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCCC"
        );
        let stream = [
            json!({"type": "rate_limit_event", "rate_limit_info": {"status": "rejected", "rateLimitType": "five_hour", "overageStatus": "allowed", "isUsingOverage": true}}),
            json!({"type": "result", "subtype": "success", "is_error": false, "result": "Reviewed."}),
        ]
        .map(|line| line.to_string());
        let directory = TempDir::new().expect("a temporary directory");
        let work = directory.path().join(TOKEN);
        std::fs::create_dir(&work).expect("a working directory");
        let provider = CliProvider::agent(
            AgentKind::Claude,
            settings(&directory, &replaying(&directory, &stream))
                .with_working_directory(&work)
                .with_credential(Credential::key("ANTHROPIC_API_KEY", TOKEN)),
        );

        let (completion, logged) =
            captured_logs(|| blocking(run(&provider, &[Message::user("Review the change.")])));

        completion.expect("a turn on usage credits");
        let credited: Vec<&str> = logged
            .lines()
            .filter(|line| line.contains("usage credits"))
            .collect();
        assert_eq!(credited.len(), 1, "{logged}");
        assert!(!credited[0].contains(TOKEN), "{logged}");
        assert!(
            credited[0].contains(&format!("directory={}", directory.path().display())),
            "{logged}"
        );
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

    /// A stand-in for Claude that records its arguments, its environment and
    /// the MCP configuration file it is given, then answers as `claude
    /// --print` does.
    fn recording_claude(arguments: &Path, environment: &Path, file: &Path) -> String {
        format!(
            r#"printf '%s\n' "$@" > '{arguments}'
env > '{environment}'
while [ $# -gt 0 ]; do
  if [ "$1" = "--mcp-config" ]; then cp "$2" '{file}'; fi
  shift
done
echo '{{"type":"result","subtype":"success","is_error":false,"result":"done"}}'"#,
            arguments = arguments.display(),
            environment = environment.display(),
            file = file.display(),
        )
    }

    const HOST_TOKEN: &str = "host-turn-notarealtoken";

    /// The host's server as a host that serves its agent every tool it may
    /// use attaches it: one remote endpoint, signed in with a token of the
    /// run's own, offering exactly the tools named, waiting as long as a
    /// turn on a call, required, and listed up front.
    fn host_server() -> McpServer {
        McpServer::remote("http://127.0.0.1:9/mcp")
            .with_header("Authorization", "Bearer ${HOST_MCP_TOKEN}")
            .with_secret("HOST_MCP_TOKEN", HOST_TOKEN)
            .with_tools(["a", "b"])
            .exclusive()
            .with_tool_timeout(Duration::from_secs(1800))
            .with_startup_timeout(Duration::from_secs(30))
            .require()
            .eager()
    }

    /// What a run of `agent` on `model` under `given` was started with: its
    /// arguments, with the rendered file's path as `<file>` and the
    /// generated variables' token as `TOKEN`, the file's contents, and the
    /// child's environment.
    async fn golden(
        agent: AgentKind,
        model: &str,
        given: impl FnOnce(CliSettings) -> CliSettings,
    ) -> (
        Vec<String>,
        Option<String>,
        std::collections::BTreeMap<String, String>,
    ) {
        let directory = TempDir::new().expect("a temporary directory");
        let captured = directory.path().join("arguments");
        let recorded = directory.path().join("environment");
        let copied = directory.path().join("mcp.json");
        let script = match agent {
            AgentKind::Claude => recording_claude(&captured, &recorded, &copied),
            AgentKind::Codex => recording_codex(&captured, &recorded),
        };
        let provider = CliProvider::agent(agent, given(settings(&directory, &script)));

        let execution = provider
            .execute_prompt(Some(model), "hi", "golden")
            .await
            .expect("an execution");
        assert_eq!(execution.status, ExitStatus::Code(0), "{execution:?}");

        let mut arguments = recorded_arguments(&captured);
        if let Some(at) = arguments
            .iter()
            .position(|argument| argument == "--mcp-config")
        {
            arguments[at + 1] = "<file>".to_string();
        }
        let arguments = arguments
            .iter()
            .map(|argument| untokened(argument))
            .collect();
        let file = std::fs::read_to_string(&copied)
            .ok()
            .map(|file| untokened(&file));
        let environment = recorded_environment(&recorded)
            .into_iter()
            .map(|(variable, value)| (untokened(&variable), value))
            .collect();
        (arguments, file, environment)
    }

    const GOLDEN_FILE: &str = r#"{
  "mcpServers": {
    "host": {
      "headers": {
        "Authorization": "${ABNEGATE_MCP_TOKEN_0}"
      },
      "type": "http",
      "url": "http://127.0.0.1:9/mcp"
    }
  }
}"#;

    const GOLDEN_OVERRIDE: &str = r#"mcp_servers.host={ "url" = "http://127.0.0.1:9/mcp", "env_http_headers" = { "Authorization" = "ABNEGATE_MCP_TOKEN_0" }, "enabled_tools" = ["a", "b"], "tool_timeout_sec" = 1800, "startup_timeout_sec" = 30, "default_tools_approval_mode" = "approve", "required" = true, "omit_tools_from" = ["deferred"] }"#;

    /// Everything a host-shaped server attached as `host` gives each agent,
    /// with the agent's own tools granted and withheld, pinned whole.
    ///
    /// Each flag and key has its counterpart in what a host driving the
    /// CLIs itself would pass: isolation, strict MCP loading, the server and
    /// its exact tools, the permission prompt on a granted Claude run, the
    /// tool timeout in both of Claude's variables, and for Codex the strict
    /// configuration, every withheld tool, the read-only sandbox and each of
    /// the server's keys. The only differences are in how the same thing is
    /// written:
    ///
    /// - Claude reads the server from a private file whose path is passed,
    ///   not from inline JSON;
    /// - each allowed tool gets an `--allowedTools` of its own, not one comma
    ///   list;
    /// - the token reaches the server through a generated variable, which
    ///   the file's header refers to for Claude and `env_http_headers` names
    ///   for Codex, not through a fixed variable and `bearer_token_env_var`;
    /// - Codex is given the server as one inline table, not one `-c` per key;
    /// - the options come after the streaming flags and before the model,
    ///   and Codex's server after its own flags and before its sandbox.
    #[tokio::test]
    async fn a_host_shaped_server_reaches_each_agent_exactly_as_pinned() {
        let isolation = [
            "--setting-sources",
            "",
            "--settings",
            r#"{"crossSessionInbound":"refuse"}"#,
        ];
        let allowed = [
            "--allowedTools",
            "mcp__host__a",
            "--allowedTools",
            "mcp__host__b",
        ];

        let (granted, file, environment) = golden(AgentKind::Claude, "opus", |settings| {
            settings
                .isolate()
                .with_mcp_server("host", host_server())
                .with_permission_prompt("host", "approve_tool_call")
        })
        .await;
        let mut expected = vec![
            "--verbose",
            "--output-format",
            "stream-json",
            "--mcp-config",
            "<file>",
            "--strict-mcp-config",
        ];
        expected.extend(isolation);
        expected.extend(allowed);
        expected.extend([
            "--permission-prompt-tool",
            "mcp__host__approve_tool_call",
            "--model",
            "opus",
            "--print",
        ]);
        assert_eq!(granted, expected);
        assert_eq!(file.as_deref(), Some(GOLDEN_FILE));
        for (variable, value) in [
            ("MCP_TOOL_TIMEOUT", "1800000"),
            ("CLAUDE_CODE_MCP_TOOL_IDLE_TIMEOUT", "1800000"),
            ("DISABLE_AUTOUPDATER", "1"),
            ("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1"),
            ("CLAUDE_CODE_DISABLE_AUTO_MEMORY", "1"),
        ] {
            assert_eq!(
                environment.get(variable).map(String::as_str),
                Some(value),
                "{variable}"
            );
        }
        let bearer = format!("Bearer {HOST_TOKEN}");
        assert_eq!(
            environment.get("ABNEGATE_MCP_TOKEN_0").map(String::as_str),
            Some(bearer.as_str())
        );
        assert!(
            !environment.contains_key("HOST_MCP_TOKEN"),
            "{environment:?}"
        );

        let (withheld, file, _) = golden(AgentKind::Claude, "opus", |settings| {
            settings
                .isolate()
                .withhold_tools()
                .with_mcp_server("host", host_server())
        })
        .await;
        let mut expected = vec![
            "--verbose",
            "--output-format",
            "stream-json",
            "--mcp-config",
            "<file>",
            "--strict-mcp-config",
        ];
        expected.extend(isolation);
        expected.extend(["--tools", ""]);
        expected.extend(allowed);
        expected.extend(["--model", "opus", "--print"]);
        assert_eq!(withheld, expected);
        assert_eq!(file.as_deref(), Some(GOLDEN_FILE));

        let strict = [
            "--ignore-user-config",
            "--disable",
            "apps",
            "--disable",
            "plugins",
        ];
        let (granted, file, environment) = golden(AgentKind::Codex, "gpt-6-sol", |settings| {
            settings
                .isolate()
                .with_mcp_server("host", host_server())
                .with_sandbox(CodexSandbox::WorkspaceWrite)
        })
        .await;
        let mut expected = vec!["exec", "--json", "--skip-git-repo-check"];
        expected.extend(strict);
        expected.extend([
            "-c",
            GOLDEN_OVERRIDE,
            "--sandbox",
            "workspace-write",
            "--model",
            "gpt-6-sol",
            "-",
        ]);
        assert_eq!(granted, expected);
        assert_eq!(file, None);
        assert_eq!(
            environment.get("ABNEGATE_MCP_TOKEN_0").map(String::as_str),
            Some(bearer.as_str())
        );
        for variable in ["MCP_TOOL_TIMEOUT", "DISABLE_AUTOUPDATER"] {
            assert!(!environment.contains_key(variable), "{variable}");
        }

        let (withheld, _, _) = golden(AgentKind::Codex, "gpt-6-sol", |settings| {
            settings
                .isolate()
                .withhold_tools()
                .with_mcp_server("host", host_server())
        })
        .await;
        let mut expected = vec!["exec", "--json", "--skip-git-repo-check"];
        expected.extend(strict);
        expected.extend([
            "--disable",
            "shell_tool",
            "--disable",
            "view_image",
            "--disable",
            "goals",
            "--disable",
            "sleep_tool",
            "--disable",
            "image_generation",
            "-c",
            "agents.enabled=false",
            "-c",
            r#"web_search="disabled""#,
            "-c",
            "tools.experimental_request_user_input.enabled=false",
            "-c",
            GOLDEN_OVERRIDE,
            "--sandbox",
            "read-only",
            "--model",
            "gpt-6-sol",
            "-",
        ]);
        assert_eq!(withheld, expected);
    }
}
