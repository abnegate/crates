use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use super::Session;
use crate::Application;

const DEFAULT_MAX_FILE_SIZE: usize = 10 * 1024 * 1024;
const DEFAULT_COMMAND_TIMEOUT: Duration = Duration::from_secs(300);

/// Where and how a tool call runs.
///
/// Confinement to [`working_directory`](Self::working_directory) is the file
/// tools' alone. `run_command` holds a command to its allow-list and its
/// arguments to no shell syntax, but the programs on that list build, test
/// and hook code from the tree, so a call to it is arbitrary execution on the
/// host. What gates it is its [`Tier::Host`](super::Tier::Host): the loop puts
/// it to the user through [`AgentCallback::approve`](crate::AgentCallback)
/// before it runs.
#[derive(Debug, Clone)]
pub struct ToolContext {
    /// The root file tools stay beneath and commands run in by default.
    pub working_directory: PathBuf,
    /// The whole environment a spawned child is given. It is an allowlist:
    /// nothing from the host process reaches a child unless it is named here.
    pub env: HashMap<String, String>,
    /// Largest file, in bytes, a tool will read into memory.
    pub max_file_size: usize,
    /// How long a command runs when its call names no limit of its own.
    pub command_timeout: Duration,
    /// Whether tools may act outside `working_directory`.
    ///
    /// Off by default: file tools stay inside the working directory. On, they
    /// address the host directly and paths are taken at face value. Only turn
    /// this on where the caller has asked for it and knows what it means.
    pub unrestricted: bool,
    /// Which chat or task run this tool call belongs to.
    pub session: Session,
    /// The name the tools keep their own files under, as `.{application}/`
    /// inside `working_directory`: background job logs live in
    /// `.{application}/jobs`.
    pub application: Application,
}

impl Default for ToolContext {
    fn default() -> Self {
        Self {
            working_directory: std::env::current_dir().unwrap_or_default(),
            env: std::env::vars().collect(),
            max_file_size: DEFAULT_MAX_FILE_SIZE,
            command_timeout: DEFAULT_COMMAND_TIMEOUT,
            unrestricted: false,
            session: Session::Detached,
            application: Application::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_context_default() {
        let context = ToolContext::default();
        assert!(
            context.working_directory.exists() || context.working_directory.as_os_str().is_empty()
        );
        assert_eq!(context.max_file_size, 10 * 1024 * 1024);
        assert_eq!(context.command_timeout, Duration::from_secs(300));
        assert_eq!(context.application, Application::default());
    }
}
