use super::AgentPhase;
use crate::tools::ToolResult;

/// Progress reports from a running [`Agent`](super::Agent).
pub trait AgentCallback: Send + Sync {
    /// The agent moved to another phase.
    fn on_phase_change(&self, phase: AgentPhase, message: Option<&str>);

    /// A tool is about to run with these arguments.
    fn on_tool_call(&self, tool_name: &str, arguments: &str);

    /// A tool returned.
    fn on_tool_result(&self, tool_name: &str, result: &ToolResult);

    /// The agent produced its answer.
    fn on_response(&self, response: &str);
}
