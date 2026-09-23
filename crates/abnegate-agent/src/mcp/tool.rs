use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use rmcp::model::CallToolRequestParams;
use rmcp::model::CallToolResult;
use rmcp::model::JsonObject;
use serde_json::Value;
use tokio::time::timeout;

use super::format::format_call_result;
use super::session::McpSession;
use crate::tools::MAX_TOOL_OUTPUT_CHARACTERS;
use crate::tools::TIMEOUT_SLACK;
use crate::tools::Tier;
use crate::tools::Tool;
use crate::tools::ToolContext;
use crate::tools::ToolError;
use crate::tools::ToolResult;
use crate::tools::trim_middle;

const MAX_MCP_OUTPUT_CHARACTERS: usize = MAX_TOOL_OUTPUT_CHARACTERS;

/// One tool advertised by a connected MCP server.
pub struct McpTool {
    qualified_name: String,
    remote_name: String,
    description: String,
    parameters_schema: Value,
    session: Arc<McpSession>,
}

impl McpTool {
    pub(super) fn new(
        qualified_name: String,
        remote_name: String,
        description: String,
        parameters_schema: Value,
        session: Arc<McpSession>,
    ) -> Self {
        Self {
            qualified_name,
            remote_name,
            description,
            parameters_schema,
            session,
        }
    }
}

#[async_trait]
impl Tool for McpTool {
    fn name(&self) -> &str {
        &self.qualified_name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn parameters_schema(&self) -> Value {
        self.parameters_schema.clone()
    }

    /// An attached server is third-party code, and nothing that reaches this
    /// client distinguishes a remote lookup from a remote publication. The
    /// tool annotations that would are hints the server writes about itself,
    /// which the MCP specification says a client must not make tool use
    /// decisions from. So every remote method is treated as the one that
    /// cannot be recalled, and the reader sees it before it runs.
    fn tier(&self) -> Tier {
        Tier::Outward
    }

    /// The call's own limit, the configured command timeout, with room to
    /// report it.
    fn timeout(&self, context: &ToolContext) -> Duration {
        context.command_timeout + TIMEOUT_SLACK
    }

    async fn execute(
        &self,
        parameters: Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let arguments = json_object(parameters)?;
        let request =
            CallToolRequestParams::new(self.remote_name.clone()).with_arguments(arguments);

        let call = self.session.call(request);
        let result = match timeout(context.command_timeout, call).await {
            Ok(Ok(result)) => result,
            Ok(Err(error)) => return Ok(ToolResult::error(error.to_string())),
            Err(_) => {
                return Ok(ToolResult::error(format!(
                    "MCP tool '{}' timed out after {} seconds",
                    self.qualified_name,
                    context.command_timeout.as_secs()
                )));
            }
        };

        Ok(tool_result_from_call(&result))
    }
}

/// The call's result as the model reads it, keeping the start and the end of
/// a long one: an error a server reports last is the part worth reading.
fn tool_result_from_call(result: &CallToolResult) -> ToolResult {
    let output = trim_middle(&format_call_result(result), MAX_MCP_OUTPUT_CHARACTERS);
    if result.is_error.unwrap_or(false) {
        ToolResult::error(output)
    } else {
        ToolResult::success(output)
    }
}

fn json_object(parameters: Value) -> Result<JsonObject, ToolError> {
    match parameters {
        Value::Object(map) => Ok(map),
        Value::Null => Ok(JsonObject::new()),
        other => Err(ToolError::InvalidParameters(format!(
            "MCP tool arguments must be a JSON object, got {other}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use rmcp::model::ContentBlock;

    use super::*;
    use crate::mcp::format::UNTRUSTED_MARKER;

    #[test]
    fn errored_call_result_keeps_the_untrusted_marker() {
        let result = tool_result_from_call(&CallToolResult::error(vec![ContentBlock::text(
            "server said no",
        )]));
        assert!(!result.success);
        let error = result.error.expect("error payload");
        assert!(error.starts_with(UNTRUSTED_MARKER), "{error}");
    }

    /// The cut used to keep only the head, which dropped the end of a long
    /// result - where a server reports what went wrong.
    #[test]
    fn huge_call_result_keeps_its_head_and_tail() {
        let text = format!("HEAD_MCP{}TAIL_MCP", "m".repeat(20_000));
        let result = tool_result_from_call(&CallToolResult::success(vec![ContentBlock::text(
            text.clone(),
        )]));
        assert!(result.success);
        let output = result.output.expect("success output");
        assert!(output.starts_with(UNTRUSTED_MARKER), "{output}");
        assert!(output.contains("HEAD_MCP"), "{output}");
        assert!(output.ends_with("TAIL_MCP"), "{output}");
        assert!(output.contains("characters trimmed"), "{output}");
        assert!(output.chars().count() <= MAX_MCP_OUTPUT_CHARACTERS);
    }

    #[test]
    fn huge_error_call_result_is_capped() {
        let result = tool_result_from_call(&CallToolResult::error(vec![ContentBlock::text(
            format!("{}the actual failure", "e".repeat(20_000)),
        )]));
        assert!(!result.success);
        let error = result.error.expect("error payload");
        assert!(error.starts_with(UNTRUSTED_MARKER), "{error}");
        assert!(error.ends_with("the actual failure"), "{error}");
        assert!(error.contains("characters trimmed"));
        assert!(error.chars().count() <= MAX_MCP_OUTPUT_CHARACTERS);
    }
}
