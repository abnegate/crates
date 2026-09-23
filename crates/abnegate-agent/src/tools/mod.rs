//! The tools an agent reaches its environment through, and the registry it
//! finds them in.
//!
//! Every tool declares its own [`Tier`], so batching and confirmation read the
//! consequences of a call from the tool rather than from a list kept in step
//! with the catalog by hand. File tools stay beneath [`ToolContext::cwd`]
//! unless the context is unrestricted, and every path they open is resolved
//! once, against a descriptor for the root, so the path that was checked is the
//! path that is opened.

mod beneath;
mod command;
mod context;
mod error;
mod file;
pub mod job;
mod reason;
mod registry;
mod result;
mod session;
pub mod tail;
mod text;
mod tier;
mod tool;
mod vision;

pub use abnegate_secret::sanitize;
pub use command::{MAX_SLEEP_SECS, RunCommandTool, RunShellTool};
pub use context::{DEFAULT_APPLICATION, ToolContext};
pub use error::ToolError;
pub use file::{ApplyPatchTool, ListFilesTool, ReadFileTool, SearchCodeTool, WriteFileTool};
pub use reason::{REASON_DESCRIPTION, REASON_PARAM, reason_property};
pub use registry::ToolRegistry;
pub use result::ToolResult;
pub use session::Session;
pub use text::{
    LINE_BREAK, MAX_PREVIEW_CHARS, MAX_TOOL_MESSAGE_CHARS, MAX_TOOL_OUTPUT_CHARS, excerpt,
};
pub use tier::{CONFIRMED_FROM, Tier};
pub use tool::Tool;
pub use vision::is_vision_url;

pub(crate) use text::{ERROR_PREFIX, trim_middle};
