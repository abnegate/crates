//! The tools an agent reaches its environment through, and the registry it
//! finds them in.
//!
//! Every tool declares its own [`Tier`], so batching and confirmation read the
//! consequences of a call from the tool rather than from a list kept in step
//! with the catalog by hand. File tools stay beneath [`ToolContext::working_directory`]
//! unless the context is unrestricted, and every path they open is resolved
//! once, against a descriptor for the root, so the path that was checked is the
//! path that is opened.

mod beneath;
mod command;
mod context;
mod error;
mod file;
pub mod job;
mod preview;
pub(crate) mod process;
mod reason;
mod registry;
mod rendering;
mod result;
mod session;
pub mod tail;
mod text;
mod tier;
mod vision;
mod wait;

use std::time::Duration;

pub use abnegate_exec::DEFAULT_ENVIRONMENT;
pub use abnegate_exec::EnvironmentPolicy;
use abnegate_llm::ToolDefinition;
pub use abnegate_secret::sanitize;
use async_trait::async_trait;
pub use command::MAXIMUM_SLEEP;
pub use command::RunCommandTool;
pub use command::RunShellTool;
pub use context::ToolContext;
pub use error::ToolError;
pub use file::ApplyPatchTool;
pub use file::ListFilesTool;
pub use file::ReadFileTool;
pub use file::SearchCodeTool;
pub use file::WriteFileTool;
pub use preview::Preview;
pub use reason::REASON_DESCRIPTION;
pub use reason::REASON_PARAMETER;
pub use reason::reason_property;
pub use registry::ToolRegistry;
pub use rendering::Rendering;
pub use result::ToolResult;
use serde_json::Value;
pub use session::Session;
pub(crate) use text::ERROR_PREFIX;
pub use text::LINE_BREAK;
pub use text::MAXIMUM_PREVIEW_CHARACTERS;
pub use text::MAXIMUM_TOOL_MESSAGE_CHARACTERS;
pub use text::MAXIMUM_TOOL_OUTPUT_CHARACTERS;
pub(crate) use text::quote;
pub(crate) use text::trim_middle;
pub(crate) use text::word;
pub use tier::CONFIRMED_FROM;
pub use tier::Tier;
pub use vision::is_vision_url;
pub use wait::WaitForTool;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// What a tool that enforces its own limit adds to it for the outer bound,
/// so the outer bound never pre-empts the inner one.
pub(crate) const TIMEOUT_SLACK: Duration = Duration::from_secs(30);

/// Something the agent can call.
#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;

    /// What the tool does, as the model reads it.
    fn description(&self) -> &str;

    /// JSON Schema for the call's arguments.
    fn parameters_schema(&self) -> Value;

    async fn execute(
        &self,
        parameters: Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError>;

    /// How long a caller should let this tool run before abandoning it.
    ///
    /// Tools that shell out enforce their own, finer limit; this is the outer
    /// bound a caller applies so that a wedged tool cannot hold a loop open
    /// indefinitely. The default suits tools that query a database.
    fn timeout(&self, _context: &ToolContext) -> Duration {
        DEFAULT_TIMEOUT
    }

    /// What a call to this tool costs if it turns out to be the wrong one.
    ///
    /// Batching and confirmation both read this, so a tool declares its
    /// consequences once and every caller agrees about them.
    fn tier(&self) -> Tier {
        Tier::Read
    }

    /// Whether the assistant's turn ends the moment this tool is called.
    ///
    /// The loop stops after it: nothing queued behind it runs, and no further
    /// model round follows, because what comes next is the user's reply rather
    /// than anything the model could say now.
    fn ends_turn(&self) -> bool {
        false
    }

    /// What this specific call will do, for the reader deciding whether to
    /// allow it.
    ///
    /// Rendered from the call's own arguments, so the reader weighs the action
    /// rather than the model's account of it, and rendered whole and
    /// verbatim: every argument shown as the call holds it, blank space,
    /// blank lines and indentation included, never squeezed. The
    /// [`Preview`] built from it escapes what cannot be drawn
    /// as itself, holds it to a length and says so when it does. Call text
    /// whose extent the reader has to see, a command or the directory it runs
    /// in, goes in a [code span](Rendering::code), which no backtick it
    /// holds can close. A tool that leaves this alone is shown as the call
    /// itself, its name and every argument.
    fn preview(&self, _parameters: &Value) -> Option<Rendering> {
        None
    }

    /// The OpenAI-style function definition the model is offered.
    fn to_definition(&self) -> ToolDefinition {
        ToolDefinition::function(self.name(), self.description(), self.parameters_schema())
    }
}
