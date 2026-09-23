use super::{AgentCallback, AgentPhase};
use crate::tools::ToolResult;

/// A callback for a caller that does not track progress.
///
/// Keeps the default [`approve`](AgentCallback::approve): nobody is there to
/// confirm a call, so every confirmed tier is refused.
pub struct NoOpCallback;

impl AgentCallback for NoOpCallback {
    fn on_phase_change(&self, _phase: AgentPhase, _message: Option<&str>) {}
    fn on_tool_call(&self, _tool_name: &str, _arguments: &str) {}
    fn on_tool_result(&self, _tool_name: &str, _result: &ToolResult) {}
    fn on_response(&self, _response: &str) {}
}
