use async_trait::async_trait;
use rmcp::model::{CallToolRequestParams, CallToolResult, JsonObject};
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::timeout;

use super::format::format_call_result;
use super::session::McpSession;
use crate::tools::{Tier, Tool, ToolContext, ToolError, ToolResult};

const MAX_MCP_OUTPUT_CHARS: usize = 8_000;
const TRUNCATION_MARKER: &str = "\n[truncated]";

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

    /// The method and the arguments it was given.
    ///
    /// A remote method has no catalog entry for a reader to recognise it by,
    /// so the call itself is the whole of what there is to show them.
    fn preview(&self, params: &Value) -> Option<String> {
        let arguments = params
            .as_object()
            .filter(|object| !object.is_empty())
            .and_then(|object| serde_json::to_string(object).ok());
        Some(match arguments {
            Some(arguments) => format!("Call `{}` with {arguments}.", self.qualified_name),
            None => format!("Call `{}` with no arguments.", self.qualified_name),
        })
    }

    async fn execute(&self, params: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let arguments = json_object(params)?;
        let request =
            CallToolRequestParams::new(self.remote_name.clone()).with_arguments(arguments);

        let call = self.session.call(request);
        let result = match timeout(Duration::from_secs(context.command_timeout), call).await {
            Ok(Ok(result)) => result,
            Ok(Err(error)) => return Ok(ToolResult::error(error.to_string())),
            Err(_) => {
                return Ok(ToolResult::error(format!(
                    "MCP tool '{}' timed out after {} seconds",
                    self.qualified_name, context.command_timeout
                )));
            }
        };

        Ok(tool_result_from_call(&result))
    }
}

fn tool_result_from_call(result: &CallToolResult) -> ToolResult {
    let output = truncate_chars(&format_call_result(result), MAX_MCP_OUTPUT_CHARS);
    if result.is_error.unwrap_or(false) {
        ToolResult::error(output)
    } else {
        ToolResult::success(output)
    }
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    match text.char_indices().nth(max_chars) {
        Some((index, _)) => format!("{}{TRUNCATION_MARKER}", &text[..index]),
        None => text.to_string(),
    }
}

fn json_object(params: Value) -> Result<JsonObject, ToolError> {
    match params {
        Value::Object(map) => Ok(map),
        Value::Null => Ok(JsonObject::new()),
        other => Err(ToolError::InvalidParams(format!(
            "MCP tool arguments must be a JSON object, got {other}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::format::UNTRUSTED_MARKER;
    use rmcp::model::ContentBlock;

    #[test]
    fn errored_call_result_keeps_the_untrusted_marker() {
        let result = tool_result_from_call(&CallToolResult::error(vec![ContentBlock::text(
            "server said no",
        )]));
        assert!(!result.success);
        let error = result.error.expect("error payload");
        assert!(error.starts_with(UNTRUSTED_MARKER), "{error}");
    }

    #[test]
    fn huge_call_result_is_capped_before_tool_result() {
        let text = format!("HEAD_MCP{}TAIL_MCP", "m".repeat(20_000));
        let result = tool_result_from_call(&CallToolResult::success(vec![ContentBlock::text(
            text.clone(),
        )]));
        assert!(result.success);
        let output = result.output.expect("success output");
        assert!(output.starts_with(UNTRUSTED_MARKER), "{output}");
        assert!(output.contains("HEAD_MCP"), "{output}");
        assert!(!output.contains("TAIL_MCP"), "{output}");
        assert!(output.contains("[truncated]"), "{output}");
        assert!(output.chars().count() <= MAX_MCP_OUTPUT_CHARS + 32);
        assert!(output.chars().count() < text.chars().count());
    }

    #[test]
    fn huge_error_call_result_is_capped() {
        let result = tool_result_from_call(&CallToolResult::error(vec![ContentBlock::text(
            "e".repeat(20_000),
        )]));
        assert!(!result.success);
        let error = result.error.expect("error payload");
        assert!(error.starts_with(UNTRUSTED_MARKER), "{error}");
        assert!(error.contains("eeee"));
        assert!(error.contains("[truncated]"));
        assert!(error.chars().count() <= MAX_MCP_OUTPUT_CHARS + 32);
    }
}
