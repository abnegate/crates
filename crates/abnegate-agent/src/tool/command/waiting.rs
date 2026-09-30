use async_trait::async_trait;
use serde_json::Value;

use crate::tool::Tool;
use crate::tool::ToolContext;
use crate::tool::ToolError;
use crate::tool::ToolResult;
use crate::tool::WaitFor;

/// A shell tool, which tells the model to wait for the jobs it starts, and so
/// tells a turn that is offered `wait_for` one thing and a turn that is not
/// another.
#[async_trait]
pub(super) trait Waiting: Tool + Sized + 'static {
    fn schema(wait_for: WaitFor) -> Value;

    async fn run(
        &self,
        parameters: Value,
        context: &ToolContext,
        wait_for: WaitFor,
    ) -> Result<ToolResult, ToolError>;
}
