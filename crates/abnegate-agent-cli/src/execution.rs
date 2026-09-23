//! Everything one run of an agent produced.

use abnegate_llm::ExitStatus;

use crate::log::ExecutionLogFiles;
use crate::stdout_parse_result::StdoutParseResult;

/// One run of an agent, before it is judged a success or a failure.
///
/// [`CliProvider::execute`](crate::CliProvider::execute) returns this for any
/// run that started and ended on its own or was stopped after the agent
/// reported a failure, so a caller that needs more than a
/// [`Completion`](abnegate_llm::Completion) — the schema-shaped answer, the
/// cost, the session to resume — can read it and decide for itself.
#[derive(Debug, Clone)]
pub struct Execution {
    pub stdout: StdoutParseResult,
    /// The agent's diagnostics, with any credential it echoed redacted.
    pub stderr: String,
    pub status: ExitStatus,
    pub log: Option<ExecutionLogFiles>,
}
