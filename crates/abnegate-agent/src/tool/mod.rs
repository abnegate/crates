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
#[expect(clippy::module_inception)]
mod tool;
mod vision;
mod wait;

pub use abnegate_secret::sanitize;
pub use command::MAX_SLEEP_SECONDS;
pub use command::RunCommandTool;
pub use command::RunShellTool;
pub use context::ToolContext;
pub use environment::DEFAULT_ENVIRONMENT;
pub use environment::EnvironmentPolicy;
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
pub use session::Session;
pub(crate) use text::ERROR_PREFIX;
pub use text::LINE_BREAK;
pub use text::MAX_PREVIEW_CHARACTERS;
pub use text::MAX_TOOL_MESSAGE_CHARACTERS;
pub use text::MAX_TOOL_OUTPUT_CHARACTERS;
pub(crate) use text::quote;
pub(crate) use text::trim_middle;
pub(crate) use text::word;
pub use tier::CONFIRMED_FROM;
pub use tier::Tier;
pub(crate) use tool::TIMEOUT_SLACK;
pub use tool::Tool;
pub use vision::is_vision_url;
pub use wait::WaitForTool;

pub use crate::application::DEFAULT_APPLICATION;
