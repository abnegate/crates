use abnegate_llm::ToolCall;
use async_trait::async_trait;

use super::AgentPhase;
use crate::tools::Tier;
use crate::tools::ToolResult;

/// Progress reports from a running [`Agent`](super::Agent), and the one
/// decision it asks its caller to make.
#[async_trait]
pub trait AgentCallback: Send + Sync {
    /// The agent moved to another phase.
    fn on_phase_change(&self, phase: AgentPhase, message: Option<&str>);

    /// A tool is about to run with these arguments.
    fn on_tool_call(&self, tool_name: &str, arguments: &str);

    /// A tool returned.
    fn on_tool_result(&self, tool_name: &str, result: &ToolResult);

    /// The agent produced its answer.
    fn on_response(&self, response: &str);

    /// Whether `call`, whose tool declares `tier`, may run.
    ///
    /// Asked before every call, after [`on_tool_call`](Self::on_tool_call).
    /// A refused call does not run and the model is told it was refused.
    ///
    /// Awaited, because the answer usually comes from a person: an
    /// application puts the call to its user and waits here for the reply,
    /// without holding a runtime thread while it does. The run is paused
    /// until this returns.
    ///
    /// The default refuses every tier that is
    /// [confirmed](Tier::confirmed) - host writes, commands, anything
    /// outward - and allows the rest, so an agent nobody is watching can read
    /// but not act. An application that offers such tools implements this to
    /// put the call to its user, using
    /// [`ToolRegistry::preview`](crate::ToolRegistry::preview) to show what it
    /// will do.
    async fn approve(&self, call: &ToolCall, tier: Tier) -> bool {
        let _ = call;
        !tier.confirmed()
    }
}
