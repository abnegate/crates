mod hunk;
mod parameters;

use std::path::Path;

use async_trait::async_trait;
use hunk::PatchHunk;
pub(super) use parameters::ApplyPatchParameters;
use serde_json::Value;
use serde_json::json;

use super::blocking;
use super::read_text;
use crate::tools::REASON_PARAMETER;
use crate::tools::Tier;
use crate::tools::Tool;
use crate::tools::ToolContext;
use crate::tools::ToolError;
use crate::tools::ToolResult;
use crate::tools::beneath;
use crate::tools::reason_property;

/// Replace exact text in an existing file without rewriting the rest.
pub struct ApplyPatchTool;

#[async_trait]
impl Tool for ApplyPatchTool {
    fn name(&self) -> &str {
        "apply_patch"
    }

    fn description(&self) -> &str {
        "Edit an existing file by replacing exact text. old_string must match uniquely unless replace_all is true. Prefer this over write_file for changes to existing files. Rejected when the text does not match."
    }

    fn tier(&self) -> Tier {
        Tier::Host
    }

    /// Every replacement, what it takes out and what it puts in, since what
    /// goes in is the part of an edit a reader is deciding on.
    fn preview(&self, parameters: &Value) -> Option<String> {
        let parameters: ApplyPatchParameters = serde_json::from_value(parameters.clone()).ok()?;
        let hunks = parameters.hunks().ok()?;
        let scope = match parameters.replace_all {
            true => "every occurrence of ",
            false => "",
        };
        let replacements = hunks
            .iter()
            .map(|hunk| {
                format!(
                    "replace {scope}\"{}\" with \"{}\"",
                    hunk.old_string, hunk.new_string
                )
            })
            .collect::<Vec<String>>()
            .join("; ");
        Some(format!("Edit {}: {replacements}.", parameters.path))
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to an existing file (relative to working directory)"
                },
                "old_string": {
                    "type": "string",
                    "description": "Exact text to find. Include enough surrounding lines to make the match unique."
                },
                "new_string": {
                    "type": "string",
                    "description": "Replacement text"
                },
                "hunks": {
                    "type": "array",
                    "description": "Multiple replacements applied in order. Use instead of repeating apply_patch.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "old_string": { "type": "string" },
                            "new_string": { "type": "string" }
                        },
                        "required": ["old_string", "new_string"]
                    }
                },
                "replace_all": {
                    "type": "boolean",
                    "description": "Replace every occurrence of each old_string (default false)"
                },
                REASON_PARAMETER: reason_property()
            },
            "required": ["path", REASON_PARAMETER]
        })
    }

    async fn execute(
        &self,
        parameters: Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let parameters: ApplyPatchParameters = serde_json::from_value(parameters)
            .map_err(|error| ToolError::InvalidParameters(error.to_string()))?;
        let hunks = parameters.hunks()?;

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

        let path = parameters.path.clone();
        let context = context.clone();
        let total = blocking(move || patch(&context, &parameters, &hunks)).await?;
        Ok(ToolResult::success(format!(
            "Updated {path} ({total} replacement{})",
            if total == 1 { "" } else { "s" }
        )))
    }
}

/// Apply `hunks` to the file the call names and write it back in one step,
/// returning how many replacements were made.
fn patch(
    context: &ToolContext,
    parameters: &ApplyPatchParameters,
    hunks: &[PatchHunk],
) -> Result<usize, ToolError> {
    let path = Path::new(&parameters.path);
    let mut content = read_text(context, path)?;
    let mut total = 0;

    for (index, hunk) in hunks.iter().enumerate() {
        let matches = content.matches(&hunk.old_string).count();
        if matches == 0 {
            return Err(ToolError::Execution(format!(
                "Hunk {} did not match any text in {}. Read the file and copy the exact text to replace.",
                index + 1,
                parameters.path
            )));
        }
        if matches > 1 && !parameters.replace_all {
            return Err(ToolError::Execution(format!(
                "Hunk {} matched {} times in {}. Include more surrounding context so the match is unique, or set replace_all=true.",
                index + 1,
                matches,
                parameters.path
            )));
        }
        content = if parameters.replace_all {
            content.replace(&hunk.old_string, &hunk.new_string)
        } else {
            content.replacen(&hunk.old_string, &hunk.new_string, 1)
        };
        total += matches;
    }

    beneath::replace(context, path, content.as_bytes())?;
    Ok(total)
}
