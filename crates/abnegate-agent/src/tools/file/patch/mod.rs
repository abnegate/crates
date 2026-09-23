mod hunk;
mod params;

pub(super) use params::ApplyPatchParams;

use async_trait::async_trait;
use serde_json::{Value, json};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

use crate::tools::beneath::{self, Access};
use crate::tools::{
    REASON_PARAM, Tier, Tool, ToolContext, ToolError, ToolResult, excerpt, reason_property,
};

/// How much of a patch's first hunk an approval preview quotes.
const PATCH_HUNK_CHARS: usize = 80;

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

    fn preview(&self, params: &Value) -> Option<String> {
        let params: ApplyPatchParams = serde_json::from_value(params.clone()).ok()?;
        let hunks = params.hunks().ok()?;
        let first = excerpt(&hunks[0].old_string, PATCH_HUNK_CHARS);
        let scope = match params.replace_all {
            true => "every occurrence of ",
            false => "",
        };
        let rest = match hunks.len() {
            1 => String::new(),
            all => format!(" and {} more", all - 1),
        };
        Some(format!(
            "Edit {}: replace {scope}\"{first}\"{rest}.",
            params.path
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
                REASON_PARAM: reason_property()
            },
            "required": ["path", REASON_PARAM]
        })
    }

    async fn execute(&self, params: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let params: ApplyPatchParams = serde_json::from_value(params)
            .map_err(|error| ToolError::InvalidParams(error.to_string()))?;
        let hunks = params.hunks()?;

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

        let mut file = beneath::open(context, Path::new(&params.path), Access::Update)?;

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
                    params.path
                )));
            }
            if matches > 1 && !params.replace_all {
                return Err(ToolError::Execution(format!(
                    "Hunk {} matched {} times in {}. Include more surrounding context so the match is unique, or set replace_all=true.",
                    index + 1,
                    matches,
                    params.path
                )));
            }
            content = if params.replace_all {
                content.replace(&hunk.old_string, &hunk.new_string)
            } else {
                content.replacen(&hunk.old_string, &hunk.new_string, 1)
            };
            replacements.push(matches);
        }

        file.set_len(0)
            .and_then(|()| file.seek(SeekFrom::Start(0)))
            .and_then(|_| file.write_all(content.as_bytes()))
            .map_err(|error| ToolError::Execution(format!("Cannot write file: {error}")))?;

        let total: usize = replacements.iter().sum();
        Ok(ToolResult::success(format!(
            "Updated {} ({} replacement{})",
            params.path,
            total,
            if total == 1 { "" } else { "s" }
        )))
    }
}
