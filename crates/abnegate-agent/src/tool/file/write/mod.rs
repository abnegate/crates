mod parameters;

use std::io::Write;
use std::path::Path;

use async_trait::async_trait;
pub(super) use parameters::WriteFileParameters;
use serde_json::Value;
use serde_json::json;

use crate::tool::REASON_PARAMETER;
use crate::tool::Rendering;
use crate::tool::Tier;
use crate::tool::Tool;
use crate::tool::ToolContext;
use crate::tool::ToolError;
use crate::tool::ToolResult;
use crate::tool::beneath;
use crate::tool::beneath::Access;
use crate::tool::quote;
use crate::tool::reason_property;
use crate::tool::word;

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

    /// Where the text goes and the text itself, since what is written is the
    /// part of a write a reader is deciding on. Both are drawn verbatim but
    /// for the preview's escapes: indentation is what a Python block or a
    /// Makefile recipe is made of, so squeezing it would show the reader a
    /// file other than the one written.
    fn preview(&self, parameters: &Value) -> Option<Rendering> {
        let parameters: WriteFileParameters = serde_json::from_value(parameters.clone()).ok()?;
        let characters = parameters.content.chars().count();
        let rendered = match parameters.append {
            true => format!(
                "Append {characters} characters to {}: {}.",
                word(&parameters.path),
                quote(&parameters.content)
            ),
            false => format!(
                "Write {characters} characters to {}, replacing whatever is there: {}.",
                word(&parameters.path),
                quote(&parameters.content)
            ),
        };
        Some(Rendering::from(rendered))
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
                REASON_PARAMETER: reason_property()
            },
            "required": ["path", "content", REASON_PARAMETER]
        })
    }

    async fn execute(
        &self,
        parameters: Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let parameters: WriteFileParameters = serde_json::from_value(parameters)
            .map_err(|error| ToolError::InvalidParameters(error.to_string()))?;

        tracing::debug!(
            tool = self.name(),
            reason_given = parameters
                .reason
                .as_deref()
                .is_some_and(|why| !why.trim().is_empty()),
            "Running tool"
        );

        let normalized_path = parameters.path.replace('\\', "/");
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

        let path = Path::new(&parameters.path);
        if let Some(parent) = path.parent() {
            beneath::create_dir_all(context, parent)?;
        }

        let access = if parameters.append {
            Access::Append
        } else {
            Access::Replace
        };
        let mut file = beneath::open(context, path, access)?;
        file.write_all(parameters.content.as_bytes())
            .map_err(|error| ToolError::Execution(format!("Cannot write file: {error}")))?;

        let action = if parameters.append {
            "appended to"
        } else {
            "wrote"
        };
        Ok(ToolResult::success(format!(
            "Successfully {} {}",
            action, parameters.path
        )))
    }
}
