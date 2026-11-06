#![forbid(unsafe_code)]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![warn(missing_docs)]
//! Coding agent CLIs driven as child processes, behind the same
//! [`CompletionProvider`](abnegate_llm::CompletionProvider) contract as an
//! HTTP model.
//!
//! [`CliProvider`] runs `claude --output-format stream-json` or
//! `codex exec --json` with the conversation flattened into one prompt on
//! stdin, reads the agent's newline-delimited JSON as it streams, and returns
//! its final answer as a completion. Output is framed by [`Lines`] with a cap
//! on any one event, parsed by the agent's own parser in [`parser`] into
//! [`AgentEvent`]s, and folded into a [`StdoutParseResult`]. Once the stream
//! settles the run — the turn finished, the agent reported a failure, or a
//! diagnostic tripped the caller's [`CliSettings::tripwire`] — an agent that
//! does not exit by itself is stopped, and a run that fails in any way takes
//! every process the agent forked with it. The agent's group is killed while
//! the agent is still unreaped, so a kill never reaches a group that took
//! over its id. [`CliSettings::with_cleanup`] with [`Cleanup::Always`] cleans
//! up after a successful run too; [`CliSettings::with_marker`] marks the
//! agent's environment, so a cleanup also stops what the agent started in a
//! session of its own; and [`CliSettings::with_registry`] registers every run
//! in a [`GroupRegistry`](abnegate_exec::GroupRegistry), through which the
//! host can pass a Ctrl-C on to every run or stop them all.
//!
//! [`CliProvider::execute`] returns the whole [`Execution`] for a caller that
//! needs more than prose: the answer to a [`StructuredResult::SCHEMA`], the
//! session to resume, the cost, and where the run's [`log`] files are. A
//! Claude turn that succeeds reports its final answer apart from the
//! narration it streamed on the way there, as an [`AgentEvent::Answer`] kept
//! in [`StdoutParseResult::answer`]; [`StdoutParseResult::reply`] is that
//! answer, or all the prose when the agent gave none, and is what
//! [`complete`](abnegate_llm::CompletionProvider::complete) returns.
//! [`CliProvider::execute_prompt`] and [`CliProvider::stream_prompt`] send a
//! prompt's bytes unchanged instead of a flattened conversation, so Claude
//! reads a leading slash command as one; nothing in such a prompt is
//! escaped. A
//! run can start the agent's own [`Session`] under an id of the caller's
//! choosing or resume one, and confine Codex to a [`CodexSandbox`]. A
//! Claude or Codex run can attach [`mcp`] servers, and a Claude run can
//! restrict its tools, or be confined to reading its working directory with
//! [`CliSettings::read_only`], and
//! [`BlockingQuestion`] recovers a question the agent stopped to ask.
//! [`stream`] reads the raw Messages API stream the CLI is built on.
//!
//! [`CliProvider::stream`] yields a run's [`AgentEvent`]s as the agent emits
//! them, in an [`AgentStream`] that ends in the error
//! [`complete`](abnegate_llm::CompletionProvider::complete) would have
//! returned when the run failed; `complete` is that stream read to its end.
//! Each block of prose after the first starts with a paragraph break, so the
//! blocks read apart when joined, and [`activity`] is the markdown line a
//! reader can be shown for a tool the agent ran for itself.
//! [`AgentKind::ALL`] lists the agents, and [`AgentKind::named`] finds one by
//! its name.
//!
//! A run reports the context window of the model its turn ran on, and how
//! much of it the turn filled, as an [`AgentEvent::Occupancy`] kept in
//! [`StdoutParseResult::occupancy`]. Claude names the window in its result's
//! `modelUsage`. Codex writes it only to the rollout it keeps of the session
//! under its `CODEX_HOME`, or `~/.codex`, as the agent's environment names
//! them, which is read once the agent has exited by itself, never through a
//! link and only from its end; when that reports none, the window Codex
//! publishes for the model in its `models_cache.json` is taken, with nothing
//! said of the fill.
//!
//! The agent is given the
//! [`DEFAULT_ENVIRONMENT`](abnegate_exec::DEFAULT_ENVIRONMENT) names,
//! `SHELL`, `XDG_RUNTIME_DIR`, `SSL_CERT_DIR` and `NODE_EXTRA_CA_CERTS`, the
//! locale categories (`LC_CTYPE`, `LC_TIME` and the rest), the names
//! [`CliSettings::allow`] adds, a proxy among them with
//! [`CliSettings::with_proxy_variables`], and its own configuration
//! variables, each from this process's environment; its sign-in variables
//! when its credential is inherited, and never otherwise, allowed or not;
//! and what the settings hand it: public
//! [variables](CliSettings::variables), secret
//! [environment](CliSettings::environment) values, the credential, and what
//! an [`mcp`] configuration moves out of its file.
//! [`CliSettings::inherit_environment`] gives it this process's whole
//! environment in place of the names.
//! [`CliSettings::withhold_host_credentials`] keeps this process's own
//! sign-in from it whichever way it is given: no agent's sign-in variable,
//! no [cloud sign-in](CLOUD_CREDENTIALS), and no agent's configuration
//! variable it was not allowed by name. [`CliSettings::refuse_credentials`]
//! keeps every agent's sign-in variable from it, however it is allowed or
//! inherited, except its own when it signs in with the host's. Every value
//! it is handed is scrubbed
//! from what the run writes down, as written, JSON-escaped or
//! percent-encoded, except the allowlisted names, its configuration
//! variables, the public variables, the proxy bypass list and whatever it
//! inherits; credential-shaped text is redacted wherever it appears. A secret
//! the agent re-encodes any other way, such as in base64, is not recognised.
//! The agent's own tools, and every stdio MCP server it starts, can read
//! everything it is given, what an [`mcp`] configuration moves out of its
//! file included; a remote MCP server is sent nothing it is given by name,
//! only what its own configuration and the [secrets](McpServer::secrets)
//! bound to it resolve to.
//!
//! ```no_run
//! use abnegate_agent_cli::AgentKind;
//! use abnegate_agent_cli::CliProvider;
//! use abnegate_agent_cli::CliSettings;
//! use abnegate_llm::CompletionProvider;
//! use abnegate_llm::CompletionRequest;
//! use abnegate_llm::Message;
//! use abnegate_llm::RequestOptions;
//!
//! # async fn example() -> Result<(), abnegate_llm::ProviderError> {
//! let provider = CliProvider::agent(
//!     AgentKind::Claude,
//!     CliSettings::default()
//!         .with_working_directory("/path/to/repository")
//!         .read_only(),
//! );
//!
//! let completion = provider
//!     .complete(CompletionRequest::new(
//!         "sonnet",
//!         &[Message::user("What does main.rs do?")],
//!         RequestOptions::new(1024),
//!     ))
//!     .await?;
//! println!("{:?}", completion.message.content);
//! # Ok(())
//! # }
//! ```
//!
//! # Failure classification
//!
//! Nothing here decides whether a failure is worth retrying. A caller already
//! owns that judgement and makes it by reading the failure text, so a provider
//! reports what went wrong in the agent's own words, with any credential
//! scrubbed out. A turn a usage limit refused is told apart all the same: it
//! is an [`AgentEvent::Limited`], and the run fails with
//! [`ProviderError::Limited`](abnegate_llm::ProviderError::Limited), which
//! renders exactly as the agent failure it replaces and carries the
//! [`Limit`](abnegate_llm::Limit): when it lifts, which window refused the
//! turn, and whether the account's usage credits ran out.
//! [`CliProvider::execute`] keeps it in [`StdoutParseResult::limit`] in the
//! agent's words alone, with the end of the agent's stderr in
//! [`Execution::stderr`]. A Claude stream that ends on a refused window with
//! no line after it fails as an agent failure in the same words, not as a
//! limit. A Claude turn that
//! reports how much of a plan's window is spent yields an
//! [`AgentEvent::Window`], kept by name in [`StdoutParseResult::windows`],
//! under the name Claude streams it with and its
//! [`WindowKind`](abnegate_llm::WindowKind). A failure the agent reported is followed by
//! [`STDERR_HEADING`] and the last whole lines of its stderr that fit in
//! 1 KiB, since some agents give the reason only there, so a caller splits on
//! the heading to read the agent's words alone. An agent that exits with no
//! report of its own fails with the end of its stderr, and a run stopped for
//! prose past its output limit fails with a message starting with
//! [`PROSE_EXCEEDED`].
//!
//! A Claude turn's failure begins with a marker ahead of those words when
//! Claude says why it refused the request: `rate limit reached (<window>,
//! <status>)` for a refused plan window, [`CREDITS_REQUIRED`] or
//! [`LONG_CONTEXT_CREDITS_REQUIRED`] for a model or a context the account's
//! usage credits cannot fund, and [`CREDITS_UNCONFIRMED`] for such a refusal
//! Claude could not look the credits up for, which is a failure rather than
//! a limit. Every other one is a limit, and so is a failed result Claude
//! gives status 429 or words a limit in, unless it gives status 401; a Codex
//! turn that fails in a limit's words is one unless they name a 401. A
//! subagent's refusal is never the turn's: Claude hands it to the main agent,
//! which answers without it.
//! A turn the account's usage credits carry past the plan's window is logged
//! once, at info, with the run's working directory, and named in
//! [`StdoutParseResult::credits`]. A failed
//! Claude result, such as one out of turns (`error_max_turns`), fails in its
//! words and then the `errors` it lists, a line apiece, and in its subtype
//! only when it gave neither.
//!
//! # Platform support
//!
//! Unix only, like the process-group handling it borrows from
//! `abnegate-exec`.

mod activity;
mod agent_stream;
mod attachments;
mod cleanup;
mod codex_sandbox;
mod delivery;
mod diagnostics;
mod drain;
mod environment;
mod event;
mod execution;
mod execution_error;
mod kind;
mod lines;
pub mod log;
pub mod mcp;
mod occupancy;
mod outcome;
mod overlong_error;
mod paragraphs;
pub mod parser;
mod provider;
mod question;
mod reader;
mod reaper;
mod rollout;
mod run;
mod scrubber;
mod session;
mod settings;
mod settlement;
mod spawned;
mod stdout_parse_result;
pub mod stream;
mod structured_result;
#[cfg(test)]
mod test_support;
pub mod transcript;
mod tripwire;
mod verdict;

pub use crate::activity::activity;
pub use crate::agent_stream::AgentStream;
pub use crate::attachments::Attachments;
pub use crate::cleanup::Cleanup;
pub use crate::codex_sandbox::CodexSandbox;
pub use crate::delivery::Delivery;
pub use crate::event::AgentEvent;
pub use crate::execution::Execution;
pub use crate::execution_error::ExecutionError;
pub use crate::kind::AgentKind;
pub use crate::lines::Lines;
pub use crate::log::EXECUTION_LOG_PREVIEW_LIMIT;
pub use crate::log::ExecutionLogFiles;
pub use crate::log::Journal;
pub use crate::log::LOG_DIRECTORY_NAME;
pub use crate::log::Record;
pub use crate::log::default_log_directory;
pub use crate::log::preview;
pub use crate::log::resolve_log_root;
pub use crate::mcp::McpConfig;
pub use crate::mcp::McpConfigError;
pub use crate::mcp::McpServer;
pub use crate::mcp::McpTransport;
pub use crate::occupancy::Occupancy;
pub use crate::overlong_error::OverlongError;
pub use crate::parser::Turn;
pub use crate::parser::claude::CREDITS_REQUIRED;
pub use crate::parser::claude::CREDITS_UNCONFIRMED;
pub use crate::parser::claude::CliContentBlock;
pub use crate::parser::claude::CliMessage;
pub use crate::parser::claude::CliUsage;
pub use crate::parser::claude::LONG_CONTEXT_CREDITS_REQUIRED;
pub use crate::parser::claude::ModelUsage;
pub use crate::parser::claude::RateLimitReport;
pub use crate::parser::claude::StreamEvent;
pub use crate::provider::CliProvider;
pub use crate::question::BlockingQuestion;
pub use crate::session::Session;
pub use crate::settings::CLOUD_CREDENTIALS;
pub use crate::settings::CliSettings;
pub use crate::settings::DEFAULT_JOURNAL_LIMIT;
pub use crate::settings::DEFAULT_LINE_LIMIT;
pub use crate::settings::DEFAULT_OUTPUT_LIMIT;
pub use crate::settings::DEFAULT_TIMEOUT;
pub use crate::settings::READ_ONLY_OPTIONS;
pub use crate::settings::READ_ONLY_SWITCHES;
pub use crate::settings::READ_ONLY_TOOLS;
pub use crate::settings::WEB_TOOLS;
pub use crate::stdout_parse_result::StdoutParseResult;
pub use crate::stream::ApiContentBlock;
pub use crate::stream::ApiDelta;
pub use crate::stream::ApiError;
pub use crate::stream::ApiMessage;
pub use crate::stream::ApiMessageDelta;
pub use crate::stream::ApiStreamEvent;
pub use crate::structured_result::StructuredResult;
pub use crate::tripwire::Tripwire;

/// Stands between an agent's own report of a failure and the end of its
/// stderr that follows it, so a reader can tell the two apart.
pub const STDERR_HEADING: &str = "\n\nThe end of the agent's stderr:\n";

/// How the failure of a run stopped for prose past its output limit begins,
/// before it names the limit.
pub const PROSE_EXCEEDED: &str = "the agent's prose exceeded";

#[cfg(doctest)]
#[doc = include_str!("../README.md")]
struct ReadmeDoctests;
