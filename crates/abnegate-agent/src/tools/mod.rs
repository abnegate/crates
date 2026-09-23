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
mod environment;
mod error;
mod file;
pub mod job;
mod process;
mod reason;
mod registry;
mod result;
mod session;
pub mod tail;
mod text;
mod tier;
mod tool;
mod vision;

pub use crate::application::DEFAULT_APPLICATION;
pub use abnegate_secret::sanitize;
pub use command::{MAX_SLEEP_SECONDS, RunCommandTool, RunShellTool};
pub use context::ToolContext;
pub use environment::{DEFAULT_ENVIRONMENT, EnvironmentPolicy};
pub use error::ToolError;
pub use file::{ApplyPatchTool, ListFilesTool, ReadFileTool, SearchCodeTool, WriteFileTool};
pub use reason::{REASON_DESCRIPTION, REASON_PARAMETER, reason_property};
pub use registry::ToolRegistry;
pub use result::ToolResult;
pub use session::Session;
pub use text::{
    LINE_BREAK, MAX_PREVIEW_CHARACTERS, MAX_TOOL_MESSAGE_CHARACTERS, MAX_TOOL_OUTPUT_CHARACTERS,
    excerpt,
};
pub use tier::{CONFIRMED_FROM, Tier};
pub(crate) use tool::TIMEOUT_SLACK;
pub use tool::Tool;
pub use vision::is_vision_url;

pub(crate) use text::{ERROR_PREFIX, trim_middle};
