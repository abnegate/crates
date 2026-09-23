mod params;

pub(super) use params::WriteFileParams;

use async_trait::async_trait;
use serde_json::{Value, json};
use std::io::Write;
use std::path::Path;

use crate::tools::beneath::{self, Access};
use crate::tools::{REASON_PARAM, Tier, Tool, ToolContext, ToolError, ToolResult, reason_property};

/// Write content to a file
pub struct WriteFileTool;

#[async_trait]
impl Tool for WriteFileTool {
    fn name(&self) -> &str {
        "write_file"
    }

    fn description(&self) -> &str {
        "Create a new file or replace an entire file. Prefer apply_patch when editing an existing file. Creates parent directories if needed. Use append=true to append instead of overwrite."
    }

    fn tier(&self) -> Tier {
        Tier::Host
    }

    fn preview(&self, params: &Value) -> Option<String> {
        let params: WriteFileParams = serde_json::from_value(params.clone()).ok()?;
        let characters = params.content.chars().count();
        Some(match params.append {
            true => format!("Append {characters} characters to {}.", params.path),
            false => format!(
                "Write {characters} characters to {}, replacing whatever is there.",
                params.path
            ),
        })
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file to write (relative to working directory)"
                },
                "content": {
                    "type": "string",
                    "description": "Content to write to the file"
                },
                "append": {
                    "type": "boolean",
                    "description": "If true, append to file instead of overwriting"
                },
                REASON_PARAM: reason_property()
            },
            "required": ["path", "content", REASON_PARAM]
        })
    }

    async fn execute(&self, params: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let params: WriteFileParams = serde_json::from_value(params)
            .map_err(|error| ToolError::InvalidParams(error.to_string()))?;

        tracing::debug!(
            tool = self.name(),
            reason_given = params
                .reason
                .as_deref()
                .is_some_and(|why| !why.trim().is_empty()),
            "Running tool"
        );

        let normalized_path = params.path.replace('\\', "/");
        if !context.unrestricted
            && (normalized_path.contains("..")
                || normalized_path.starts_with('/')
                || normalized_path.contains("/../")
                || normalized_path.ends_with("/.."))
        {
            return Err(ToolError::Execution(
                "Path contains traversal sequences".to_string(),
            ));
        }

        let path = Path::new(&params.path);
        if let Some(parent) = path.parent() {
            beneath::create_dir_all(context, parent)?;
        }

        let access = if params.append {
            Access::Append
        } else {
            Access::Replace
        };
        let mut file = beneath::open(context, path, access)?;
        file.write_all(params.content.as_bytes())
            .map_err(|error| ToolError::Execution(format!("Cannot write file: {error}")))?;

        let action = if params.append {
            "appended to"
        } else {
            "wrote"
        };
        Ok(ToolResult::success(format!(
            "Successfully {} {}",
            action, params.path
        )))
    }
}
