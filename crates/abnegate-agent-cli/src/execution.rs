//! Everything one run of an agent produced.

use abnegate_llm::ExitStatus;

use crate::log::ExecutionLogFiles;
use crate::stdout_parse_result::StdoutParseResult;

/// One run of an agent, before it is judged a success or a failure.
///
/// [`CliProvider::execute`](crate::CliProvider::execute) returns this for any
/// run that ended on its own or was stopped after its output had settled it,
/// so a caller that needs more than a [`Completion`](abnegate_llm::Completion)
/// — the schema-shaped answer, the cost, the session to resume — can read it
/// and decide for itself.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Execution {
    /// What the agent streamed. Its failure is scrubbed of every secret the
    /// run was given; its prose is the agent's answer, left as written.
    pub stdout: StdoutParseResult,
    /// The agent's diagnostics, scrubbed of every secret the run was given.
    pub stderr: String,
    pub status: ExitStatus,
    /// Why the agent was stopped rather than left to exit, when it was: the
    /// failure it reported, the diagnostic that tripped
    /// [`CliSettings::tripwire`](crate::CliSettings::tripwire), or a finished
    /// turn followed by a process that would not exit. `status` is then the
    /// stop's, not the agent's.
    pub stopped: Option<String>,
    /// The failure that settled the run while the agent was still running —
    /// the failure it reported, or the diagnostic that tripped
    /// [`CliSettings::tripwire`](crate::CliSettings::tripwire) — kept whether
    /// the agent was then stopped or exited by itself first.
    pub failure: Option<String>,
    pub log: Option<ExecutionLogFiles>,
}
