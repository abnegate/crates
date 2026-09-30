mod parameters;

use std::path::Path;
use std::time::Duration;

use async_trait::async_trait;
use parameters::ListFilesParameters;
use serde_json::Value;
use serde_json::json;

use super::confine;
use super::resolve;
use super::walk::Visit;
use super::walk::Walk;
use crate::tool::TIMEOUT_SLACK;
use crate::tool::Tool;
use crate::tool::ToolContext;
use crate::tool::ToolError;
use crate::tool::ToolResult;

pub(super) const LIST_FILES_CAP: usize = 200;

fn push_listing(files: &mut Vec<String>, total: &mut usize, name: String) {
    *total += 1;
    if files.len() < LIST_FILES_CAP {
        files.push(name);
    }
}

/// List files in a directory
pub struct ListFilesTool;

#[async_trait]
impl Tool for ListFilesTool {
    fn name(&self) -> &str {
        "list_files"
    }

    fn description(&self) -> &str {
        "List files in a directory. Use recursive=true for subdirectories. Use pattern for glob matching."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Directory path to list (relative to working directory)"
                },
                "recursive": {
                    "type": "boolean",
                    "description": "If true, list files recursively"
                },
                "pattern": {
                    "type": "string",
                    "description": "Glob pattern to filter files (e.g., '*.rs')"
                }
            },
            "required": ["path"]
        })
    }

    fn timeout(&self, context: &ToolContext) -> Duration {
        context.search_timeout + TIMEOUT_SLACK
    }

    async fn execute(
        &self,
        parameters: Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let parameters: ListFilesParameters = serde_json::from_value(parameters)
            .map_err(|error| ToolError::InvalidParameters(error.to_string()))?;

        let full_path = resolve(&context.working_directory.join(&parameters.path));
        confine(&full_path, context)?;
        if !full_path.exists() {
            return Err(ToolError::Execution(format!(
                "Path does not exist: {}",
                parameters.path
            )));
        }

        let context = context.clone();
        tokio::task::spawn_blocking(move || list(&full_path, &parameters, &context))
            .await
            .map_err(|error| ToolError::Execution(format!("The listing did not finish: {error}")))?
    }
}

/// Every entry under `root` that `parameters` asks for, capped at
/// [`LIST_FILES_CAP`], for as long as the context's `search_timeout` allows.
fn list(
    root: &Path,
    parameters: &ListFilesParameters,
    context: &ToolContext,
) -> Result<ToolResult, ToolError> {
    let mut files = Vec::new();
    let mut total = 0;
    let mut walk = Walk::new(context.search_timeout, context);
    walk.run(root, |entry, file_type| {
        let path = entry.path();
        let relative = path.strip_prefix(root).unwrap_or(&path).display();
        if file_type.is_dir() {
            if parameters.recursive {
                return Visit::Descend;
            }
            push_listing(&mut files, &mut total, format!("{relative}/"));
            return Visit::Skip;
        }
        let name = relative.to_string();
        if matches(&name, parameters.pattern.as_deref()) {
            push_listing(&mut files, &mut total, name);
        }
        Visit::Skip
    })?;

    files.sort();

    let mut output = if files.is_empty() {
        "No files found".to_string()
    } else {
        files.join("\n")
    };
    if total > files.len() {
        output.push_str(&format!("\n[truncated; omitted={}]", total - files.len()));
    }
    if let Some(reason) = walk.stopped() {
        output.push_str(&format!("\n[listing stopped early: {reason}]"));
    }
    Ok(ToolResult::success(output))
}

/// Whether `name` fits a `*suffix`, `prefix*` or substring pattern.
fn matches(name: &str, pattern: Option<&str>) -> bool {
    let Some(glob) = pattern else {
        return true;
    };
    if let Some(suffix) = glob.strip_prefix('*') {
        name.ends_with(suffix)
    } else if let Some(prefix) = glob.strip_suffix('*') {
        name.starts_with(prefix)
    } else {
        name.contains(glob)
    }
}
