use abnegate_llm::ToolDefinition;
use async_trait::async_trait;
use serde_json::Value;
use std::time::Duration;

use super::{Tier, ToolContext, ToolError, ToolResult};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// Something the agent can call.
#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;

    /// What the tool does, as the model reads it.
    fn description(&self) -> &str;

    /// JSON Schema for the call's arguments.
    fn parameters_schema(&self) -> Value;

    async fn execute(
        &self,
        parameters: Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError>;

    /// How long a caller should let this tool run before abandoning it.
    ///
    /// Tools that shell out enforce their own, finer limit; this is the outer
    /// bound a caller applies so that a wedged tool cannot hold a loop open
    /// indefinitely. The default suits tools that query a database.
    fn timeout(&self, _context: &ToolContext) -> Duration {
        DEFAULT_TIMEOUT
    }

    /// What a call to this tool costs if it turns out to be the wrong one.
    ///
    /// Batching and confirmation both read this, so a tool declares its
    /// consequences once and every caller agrees about them.
    fn tier(&self) -> Tier {
        Tier::Read
    }

    /// Whether the assistant's turn ends the moment this tool is called.
    ///
    /// The loop stops after it: nothing queued behind it runs, and no further
    /// model round follows, because what comes next is the user's reply rather
    /// than anything the model could say now.
    fn ends_turn(&self) -> bool {
        false
    }

    /// What this specific call will do, for the reader deciding whether to
    /// allow it.
    ///
    /// Rendered from the call's own arguments, so the reader weighs the action
    /// rather than the model's account of it. Tools whose tier is never
    /// confirmed have nobody to render for and leave this alone.
    fn preview(&self, _params: &Value) -> Option<String> {
        None
    }

    /// The OpenAI-style function definition the model is offered.
    fn to_definition(&self) -> ToolDefinition {
        ToolDefinition::function(self.name(), self.description(), self.parameters_schema())
    }
}
