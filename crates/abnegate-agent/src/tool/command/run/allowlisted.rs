use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use tokio::time::Duration;

use super::DESCRIPTION;
use super::NAME;
use super::RunCommandTool;
use super::Unwaited;
use super::Waiting;
use super::run_allowed;
use crate::tool::Rendering;
use crate::tool::Tier;
use crate::tool::Tool;
use crate::tool::ToolContext;
use crate::tool::ToolError;
use crate::tool::ToolResult;
use crate::tool::WaitFor;

/// `run_command` over a caller's own list of programs, as
/// [`RunCommandTool::with_commands`] builds it.
#[derive(Clone)]
pub(super) struct Allowlisted {
    commands: Arc<[String]>,
}

impl Allowlisted {
    pub(super) fn new(commands: &[&str]) -> Self {
        Self {
            commands: commands.iter().map(|command| command.to_string()).collect(),
        }
    }
}

#[async_trait]
impl Tool for Allowlisted {
    fn name(&self) -> &str {
        NAME
    }

    fn description(&self) -> &str {
        DESCRIPTION
    }

    fn tier(&self) -> Tier {
        RunCommandTool.tier()
    }

    fn preview(&self, parameters: &Value) -> Option<Rendering> {
        RunCommandTool.preview(parameters)
    }

    fn timeout(&self, context: &ToolContext) -> Duration {
        RunCommandTool.timeout(context)
    }

    fn parameters_schema(&self) -> Value {
        Self::schema(WaitFor::Offered)
    }

    async fn execute(
        &self,
        parameters: Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        self.run(parameters, context, WaitFor::Offered).await
    }

    fn unwaited(&self) -> Option<Arc<dyn Tool>> {
        Some(Arc::new(Unwaited(self.clone())))
    }
}

#[async_trait]
impl Waiting for Allowlisted {
    fn schema(wait_for: WaitFor) -> Value {
        RunCommandTool::schema(wait_for)
    }

    async fn run(
        &self,
        parameters: Value,
        context: &ToolContext,
        wait_for: WaitFor,
    ) -> Result<ToolResult, ToolError> {
        run_allowed(&self.commands, parameters, context, wait_for).await
    }
}
