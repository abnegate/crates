use std::path::PathBuf;
use std::time::Duration;

use super::EnvironmentPolicy;
use super::Session;
use crate::Application;

const DEFAULT_MAXIMUM_FILE_SIZE: usize = 10 * 1024 * 1024;
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
#[non_exhaustive]
pub struct ToolContext {
    /// The root file tools stay beneath and commands run in by default.
    pub working_directory: PathBuf,
    /// The whole environment a spawned child is given: by default each
    /// [`DEFAULT_ENVIRONMENT`](super::DEFAULT_ENVIRONMENT) name this process
    /// has as the child starts, and nothing else.
    pub environment: EnvironmentPolicy,
    /// Largest file, in bytes, a tool will read into memory.
    pub maximum_file_size: usize,
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

impl ToolContext {
    /// The same context, rooted at `working_directory`.
    ///
    /// The context is non-exhaustive, so a caller outside this crate starts
    /// from [`Default`] and changes what it needs:
    ///
    /// ```
    /// use abnegate_agent::tool::Session;
    /// use abnegate_agent::{Application, ToolContext};
    /// use std::time::Duration;
    ///
    /// let mut context = ToolContext::default()
    ///     .within("/srv/checkout")
    ///     .with_session(Session::Detached)
    ///     .with_application(Application::new("acme")?);
    /// context.command_timeout = Duration::from_secs(60);
    /// # Ok::<(), abnegate_agent::ApplicationError>(())
    /// ```
    pub fn within(mut self, working_directory: impl Into<PathBuf>) -> Self {
        self.working_directory = working_directory.into();
        self
    }

    /// The same context, for the chat or task run `session`.
    pub fn with_session(mut self, session: Session) -> Self {
        self.session = session;
        self
    }

    /// The same context, keeping its own files under `application`.
    pub fn with_application(mut self, application: Application) -> Self {
        self.application = application;
        self
    }

    /// The same context, giving children exactly `environment`.
    pub fn with_environment(mut self, environment: EnvironmentPolicy) -> Self {
        self.environment = environment;
        self
    }

    /// The same context, giving children this process's whole environment
    /// with the variables already named laid over it.
    ///
    /// The opt-out from the allowlist: only for a caller whose own
    /// environment holds nothing a child should not see.
    pub fn inherit_environment(mut self) -> Self {
        self.environment = self.environment.inheriting();
        self
    }
}

impl Default for ToolContext {
    fn default() -> Self {
        Self {
            working_directory: std::env::current_dir().unwrap_or_default(),
            environment: EnvironmentPolicy::allowlist(),
            maximum_file_size: DEFAULT_MAXIMUM_FILE_SIZE,
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
        assert_eq!(context.maximum_file_size, 10 * 1024 * 1024);
        assert_eq!(context.command_timeout, Duration::from_secs(300));
        assert_eq!(context.application, Application::default());
        assert!(!context.environment.inherits());
    }

    /// The default used to be the process's whole environment, database URL
    /// and signing keys included, for every child a tool started.
    #[test]
    fn the_default_environment_is_the_allowlist() {
        let context = ToolContext::default();
        for name in context.environment.names() {
            assert!(super::super::DEFAULT_ENVIRONMENT.contains(&name), "{name}");
        }
        let unlisted = std::env::vars()
            .map(|(name, _)| name)
            .find(|name| !super::super::DEFAULT_ENVIRONMENT.contains(&name.as_str()));
        if let Some(name) = unlisted {
            assert!(!context.environment.contains(&name), "{name}");
        }
        assert!(
            ToolContext::default()
                .inherit_environment()
                .environment
                .inherits()
        );
    }

    #[test]
    fn debug_never_prints_an_environment_value() {
        let mut context = ToolContext::default();
        context.environment.set("API_TOKEN", "hunter2-secret");
        let printed = format!("{context:?}");
        assert!(printed.contains("API_TOKEN"), "{printed}");
        assert!(!printed.contains("hunter2-secret"), "{printed}");
    }
}
