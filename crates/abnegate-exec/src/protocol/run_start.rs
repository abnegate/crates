use std::collections::BTreeSet;
use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;
use std::time::Duration;

use serde::Deserialize;
use serde::Serialize;

use super::confinement_request::ConfinementRequest;
use super::milliseconds;

/// Starts a command.
///
/// Only the job, its workspace and its command are required; everything else
/// falls back to the runner's defaults.
///
/// `Debug` prints the names in [`environment`](Self::environment) and never
/// their values, which can carry a credential.
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[non_exhaustive]
pub struct RunStart {
    /// Names the job in every message about it
    pub job_id: String,
    /// Directory the command runs in, unless
    /// [`working_directory`](Self::working_directory) names another. It must
    /// exist.
    pub workspace: PathBuf,
    /// The program to run: a path, or a name looked up on `PATH`
    pub command: String,
    /// Arguments passed to the command
    #[serde(default, rename = "args")]
    pub arguments: Vec<String>,
    /// Variables layered over the runner's
    /// [`EnvironmentPolicy`](crate::executor::EnvironmentPolicy)
    #[serde(default, rename = "env")]
    pub environment: HashMap<String, String>,
    /// How long the command may run; absent uses the executor's
    /// [default](crate::executor::ExecutorConfig::default_timeout). Whole
    /// milliseconds on the wire.
    #[serde(
        default,
        rename = "timeout_ms",
        serialize_with = "milliseconds::serialize_optional",
        deserialize_with = "milliseconds::deserialize_optional"
    )]
    pub timeout: Option<Duration>,
    /// Ceiling on stdout and stderr together, in bytes; absent uses the
    /// executor's [default](crate::executor::ExecutorConfig::max_output_bytes)
    #[serde(default, rename = "max_output_bytes")]
    pub output_limit: Option<usize>,
    /// Directory the command starts in, when it is not the workspace
    #[serde(default, rename = "working_dir")]
    pub working_directory: Option<PathBuf>,
    /// When present the job runs under OS confinement, and fails to start
    /// if this runner cannot prove confinement works. Boxed because most
    /// jobs carry none and the request is the largest part of a message.
    #[serde(default)]
    pub confinement: Option<Box<ConfinementRequest>>,
}

impl RunStart {
    /// Run `command` as `job_id` from `workspace`, with no arguments and the
    /// runner's defaults for everything else.
    pub fn new(
        job_id: impl Into<String>,
        workspace: impl Into<PathBuf>,
        command: impl Into<String>,
    ) -> Self {
        Self {
            job_id: job_id.into(),
            workspace: workspace.into(),
            command: command.into(),
            arguments: Vec::new(),
            environment: HashMap::new(),
            timeout: None,
            output_limit: None,
            working_directory: None,
            confinement: None,
        }
    }

    /// Pass `arguments` to the command.
    pub fn with_arguments<I, S>(mut self, arguments: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.arguments = arguments.into_iter().map(Into::into).collect();
        self
    }

    /// Layer `environment` over the runner's policy.
    pub fn with_environment<I, K, V>(mut self, environment: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        self.environment = environment
            .into_iter()
            .map(|(name, value)| (name.into(), value.into()))
            .collect();
        self
    }

    /// Stop the command once it has run for `timeout`.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// Deliver at most `output_limit` bytes of stdout and stderr together.
    pub fn with_output_limit(mut self, output_limit: usize) -> Self {
        self.output_limit = Some(output_limit);
        self
    }

    /// Start the command in `working_directory` instead of the workspace.
    pub fn with_working_directory(mut self, working_directory: impl Into<PathBuf>) -> Self {
        self.working_directory = Some(working_directory.into());
        self
    }

    /// Run the command under `confinement`.
    pub fn with_confinement(mut self, confinement: ConfinementRequest) -> Self {
        self.confinement = Some(Box::new(confinement));
        self
    }
}

impl fmt::Debug for RunStart {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RunStart")
            .field("job_id", &self.job_id)
            .field("workspace", &self.workspace)
            .field("command", &self.command)
            .field("arguments", &self.arguments)
            .field(
                "environment",
                &self.environment.keys().collect::<BTreeSet<&String>>(),
            )
            .field("timeout", &self.timeout)
            .field("output_limit", &self.output_limit)
            .field("working_directory", &self.working_directory)
            .field("confinement", &self.confinement)
            .finish()
    }
}
