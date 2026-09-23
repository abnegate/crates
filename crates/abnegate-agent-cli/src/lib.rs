#![forbid(unsafe_code)]
#![cfg_attr(docsrs, feature(doc_cfg))]
//! Coding agent CLIs driven as child processes, behind the same
//! [`CompletionProvider`](abnegate_llm::CompletionProvider) contract as an
//! HTTP model.
//!
//! [`CliProvider`] runs `claude --output-format stream-json` or
//! `codex exec --json` with the conversation flattened into one prompt on
//! stdin, reads the agent's newline-delimited JSON as it streams, and returns
//! its final prose as a completion. Output is framed by [`Lines`] with a cap
//! on any one event, parsed by the agent's own parser in [`parser`] into
//! [`AgentEvent`]s, and folded into a [`StdoutParseResult`]. Once the stream
//! settles the run — the turn finished, the agent reported a failure, or a
//! diagnostic tripped the caller's [`CliSettings::tripwire`] — an agent that
//! does not exit by itself is stopped, and a run that fails in any way takes
//! every process the agent forked with it.
//!
//! [`CliProvider::execute`] returns the whole [`Execution`] for a caller that
//! needs more than prose: the answer to a [`StructuredResult::SCHEMA`], the
//! session to resume, the cost, and where the run's [`log`] files are. A
//! Claude run can attach [`mcp`] servers and restrict its tools, and
//! [`BlockingQuestion`] recovers a question the agent stopped to ask.
//! [`stream`] reads the raw Messages API stream the CLI is built on.
//!
//! ```no_run
//! use abnegate_agent_cli::{AgentKind, CliProvider, CliSettings};
//! use abnegate_llm::{CompletionProvider, CompletionRequest, Message, RequestOptions};
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
//!     .complete(CompletionRequest {
//!         model: "sonnet",
//!         messages: &[Message::user("What does main.rs do?")],
//!         tools: None,
//!         options: RequestOptions { reserved: 1024 },
//!     })
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
//! scrubbed out.
//!
//! # Platform support
//!
//! Unix only, like the process-group handling it borrows from
//! `abnegate-exec`.

mod attachments;
mod delivery;
mod diagnostics;
mod environment;
mod error;
mod event;
mod execution;
mod execution_error;
mod kind;
mod lines;
pub mod log;
pub mod mcp;
mod outcome;
pub mod parser;
mod provider;
mod question;
mod reader;
mod reaper;
mod scrubber;
mod settings;
mod stdout_parse_result;
pub mod stream;
mod structured_result;
pub mod transcript;
mod tripwire;
mod verdict;

pub use crate::attachments::Attachments;
pub use crate::delivery::Delivery;
pub use crate::error::Overlong;
pub use crate::event::AgentEvent;
pub use crate::execution::Execution;
pub use crate::execution_error::ExecutionError;
pub use crate::kind::AgentKind;
pub use crate::lines::Lines;
pub use crate::log::{
    EXECUTION_LOG_PREVIEW_LIMIT, ExecutionLogFiles, Journal, LOG_DIRECTORY_NAME, Record,
    default_log_directory, preview, resolve_log_root,
};
pub use crate::mcp::{McpAttachment, McpConfig, McpServer, McpTransport};
pub use crate::parser::claude::{
    CliContentBlock, CliMessage, CliUsage, RateLimitInfo, StreamEvent,
};
pub use crate::provider::CliProvider;
pub use crate::question::BlockingQuestion;
pub use crate::settings::{
    CliSettings, DEFAULT_JOURNAL_LIMIT, DEFAULT_LINE_LIMIT, DEFAULT_OUTPUT_LIMIT, DEFAULT_TIMEOUT,
    INHERITED_VARIABLES, READ_ONLY_OPTIONS, READ_ONLY_SWITCHES, READ_ONLY_TOOLS,
};
pub use crate::stdout_parse_result::StdoutParseResult;
pub use crate::stream::{
    ApiContentBlock, ApiDelta, ApiError, ApiMessage, ApiMessageDelta, ApiStreamEvent,
};
pub use crate::structured_result::StructuredResult;
pub use crate::tripwire::Tripwire;
