use std::collections::HashMap;
use std::path::PathBuf;

use super::Session;

/// The application name a default [`ToolContext`] keeps its own files under.
pub const DEFAULT_APPLICATION: &str = "abnegate";

const DEFAULT_MAX_FILE_SIZE: usize = 10 * 1024 * 1024;
const DEFAULT_COMMAND_TIMEOUT_SECS: u64 = 300;

/// Where and how a tool call runs.
#[derive(Debug, Clone)]
pub struct ToolContext {
    pub cwd: PathBuf,
    /// The whole environment a spawned child is given. It is an allowlist:
    /// nothing from the host process reaches a child unless it is named here.
    pub env: HashMap<String, String>,
    /// Largest file, in bytes, a tool will read into memory.
    pub max_file_size: usize,
    /// Default command timeout, in seconds.
    pub command_timeout: u64,
    /// Whether tools may act outside `cwd`.
    ///
    /// Off by default: file tools stay inside the working directory and
    /// `run_command` is held to its allow-list. On, they address the host
    /// directly and paths are taken at face value. Only turn this on where
    /// the caller has asked for it and knows what it means.
    pub unrestricted: bool,
    /// Which chat or task run this tool call belongs to.
    pub session: Session,
    /// The name the tools keep their own files under, as `.{application}/`
    /// inside `cwd`: background job logs live in `.{application}/jobs`.
    pub application: String,
}

impl Default for ToolContext {
    fn default() -> Self {
        Self {
            cwd: std::env::current_dir().unwrap_or_default(),
            env: std::env::vars().collect(),
            max_file_size: DEFAULT_MAX_FILE_SIZE,
            command_timeout: DEFAULT_COMMAND_TIMEOUT_SECS,
            unrestricted: false,
            session: Session::Detached,
            application: DEFAULT_APPLICATION.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_context_default() {
        let context = ToolContext::default();
        assert!(context.cwd.exists() || context.cwd.as_os_str().is_empty());
        assert_eq!(context.max_file_size, 10 * 1024 * 1024);
        assert_eq!(context.command_timeout, 300);
        assert_eq!(context.application, DEFAULT_APPLICATION);
    }
}
