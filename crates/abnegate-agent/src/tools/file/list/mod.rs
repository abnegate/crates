mod params;

use async_trait::async_trait;
use serde_json::{Value, json};
use std::fs;
use std::path::Path;

use super::{confine, descendable};
use crate::tools::{Tool, ToolContext, ToolError, ToolResult};
use params::ListFilesParams;

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

    async fn execute(&self, params: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let params: ListFilesParams = serde_json::from_value(params)
            .map_err(|error| ToolError::InvalidParams(error.to_string()))?;

        let full_path = context.cwd.join(&params.path);

        if !full_path.exists() {
            return Err(ToolError::Execution(format!(
                "Path does not exist: {}",
                params.path
            )));
        }

        let full_path = full_path
            .canonicalize()
            .map_err(|error| ToolError::Execution(format!("Cannot resolve path: {error}")))?;
        confine(&full_path, context)?;

        let mut files = Vec::new();
        let mut total = 0;

        fn collect_files(
            directory: &Path,
            base: &Path,
            recursive: bool,
            pattern: &Option<String>,
            files: &mut Vec<String>,
            total: &mut usize,
            context: &ToolContext,
        ) -> Result<(), ToolError> {
            let entries = fs::read_dir(directory)
                .map_err(|error| ToolError::Execution(format!("Cannot read directory: {error}")))?;

            for entry in entries {
                let entry = entry
                    .map_err(|error| ToolError::Execution(format!("Cannot read entry: {error}")))?;
                let path = entry.path();
                let relative = path.strip_prefix(base).unwrap_or(&path);

                if path.is_dir() {
                    if !recursive {
                        push_listing(files, total, format!("{}/", relative.display()));
                    } else if descendable(&path, context) {
                        collect_files(&path, base, recursive, pattern, files, total, context)?;
                    }
                } else {
                    let name = relative.display().to_string();

                    let matches = if let Some(glob) = pattern {
                        if let Some(suffix) = glob.strip_prefix('*') {
                            name.ends_with(suffix)
                        } else if let Some(prefix) = glob.strip_suffix('*') {
                            name.starts_with(prefix)
                        } else {
                            name.contains(glob)
                        }
                    } else {
                        true
                    };

                    if matches {
                        push_listing(files, total, name);
                    }
                }
            }

            Ok(())
        }

        collect_files(
            &full_path,
            &full_path,
            params.recursive,
            &params.pattern,
            &mut files,
            &mut total,
            context,
        )?;

        files.sort();

        if files.is_empty() {
            Ok(ToolResult::success("No files found"))
        } else {
            let mut output = files.join("\n");
            if total > files.len() {
                output.push_str(&format!("\n[truncated; omitted={}]", total - files.len()));
            }
            Ok(ToolResult::success(output))
        }
    }
}
