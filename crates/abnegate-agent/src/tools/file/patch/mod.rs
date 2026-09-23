mod hunk;
mod parameters;

use std::io::Read;
use std::path::Path;

use async_trait::async_trait;
pub(super) use parameters::ApplyPatchParameters;
use serde_json::Value;
use serde_json::json;

use crate::tools::REASON_PARAMETER;
use crate::tools::Tier;
use crate::tools::Tool;
use crate::tools::ToolContext;
use crate::tools::ToolError;
use crate::tools::ToolResult;
use crate::tools::beneath;
use crate::tools::beneath::Access;
use crate::tools::excerpt;
use crate::tools::reason_property;

/// How much of a patch's first hunk an approval preview quotes.
const PATCH_HUNK_CHARACTERS: usize = 80;

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

    fn preview(&self, parameters: &Value) -> Option<String> {
        let parameters: ApplyPatchParameters = serde_json::from_value(parameters.clone()).ok()?;
        let hunks = parameters.hunks().ok()?;
        let first = excerpt(&hunks[0].old_string, PATCH_HUNK_CHARACTERS);
        let scope = match parameters.replace_all {
            true => "every occurrence of ",
            false => "",
        };
        let rest = match hunks.len() {
            1 => String::new(),
            all => format!(" and {} more", all - 1),
        };
        Some(format!(
            "Edit {}: replace {scope}\"{first}\"{rest}.",
            parameters.path
        ))
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

        let path = Path::new(&parameters.path);
        let mut file = beneath::open(context, path, Access::Read)?;

        let metadata = file
            .metadata()
            .map_err(|error| ToolError::Execution(format!("Cannot read file: {error}")))?;
        if metadata.len() > context.max_file_size as u64 {
            return Err(ToolError::Execution(format!(
                "File too large ({} bytes, max {})",
                metadata.len(),
                context.max_file_size
            )));
        }

        let mut content = String::new();
        file.read_to_string(&mut content)
            .map_err(|error| ToolError::Execution(format!("Cannot read file: {error}")))?;
        let mut replacements = Vec::new();

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
            replacements.push(matches);
        }

        drop(file);
        beneath::replace(context, path, content.as_bytes())?;

        let total: usize = replacements.iter().sum();
        Ok(ToolResult::success(format!(
            "Updated {} ({} replacement{})",
            parameters.path,
            total,
            if total == 1 { "" } else { "s" }
        )))
    }
}
