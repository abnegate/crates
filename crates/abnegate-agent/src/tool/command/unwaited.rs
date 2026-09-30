use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;

use super::Waiting;
use crate::tool::Rendering;
use crate::tool::Tier;
use crate::tool::Tool;
use crate::tool::ToolContext;
use crate::tool::ToolError;
use crate::tool::ToolResult;
use crate::tool::WaitFor;

/// A shell tool as a turn that is not offered `wait_for` is served it.
pub(super) struct Unwaited<T>(pub(super) T);

#[async_trait]
impl<T: Waiting> Tool for Unwaited<T> {
    fn name(&self) -> &str {
        self.0.name()
    }

    fn description(&self) -> &str {
        self.0.description()
    }

    fn parameters_schema(&self) -> Value {
        T::schema(WaitFor::Withheld)
    }

    async fn execute(
        &self,
        parameters: Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        self.0.run(parameters, context, WaitFor::Withheld).await
    }

    fn timeout(&self, context: &ToolContext) -> Duration {
        self.0.timeout(context)
    }

    fn tier(&self) -> Tier {
        self.0.tier()
    }

    fn ends_turn(&self) -> bool {
        self.0.ends_turn()
    }

    fn preview(&self, parameters: &Value) -> Option<Rendering> {
        self.0.preview(parameters)
    }
}
