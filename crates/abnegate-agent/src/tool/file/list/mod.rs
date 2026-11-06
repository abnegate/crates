mod parameters;

use std::path::Path;
use std::time::Duration;
use std::time::Instant;

use async_trait::async_trait;
use parameters::ListFilesParameters;
use serde_json::Value;
use serde_json::json;

use super::confine;
use super::resolve;
use super::walk::UNREADABLE;
use super::walk::Visit;
use super::walk::Walk;
use super::walk::walking;
use crate::tool::TIMEOUT_SLACK;
use crate::tool::Tool;
use crate::tool::ToolContext;
use crate::tool::ToolError;
use crate::tool::ToolResult;

pub(super) const LIST_FILES_CAP: usize = 200;

const OPERATION: &str = "listing";

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
        let deadline = Instant::now() + context.search_timeout;
        let parameters: ListFilesParameters = serde_json::from_value(parameters)
            .map_err(|error| ToolError::InvalidParameters(error.to_string()))?;

        let full_path = resolve(&context.working_directory.join(&parameters.path))?;
        confine(&full_path, context)?;
        if !full_path.exists() {
            return Err(ToolError::Execution(format!(
                "Path does not exist: {}",
                parameters.path
            )));
        }

        walking(OPERATION, deadline, context, move |walk, _| {
            list(walk, &full_path, &parameters)
        })
        .await?
    }
}

/// Every entry under `root` that `parameters` asks for, capped at
/// [`LIST_FILES_CAP`], for as long as `walk` goes on.
///
/// A listing that stopped early, or could not read part of the tree, says
/// so, and when it found nothing it claims only that nothing was found so
/// far.
fn list(
    walk: &mut Walk,
    root: &Path,
    parameters: &ListFilesParameters,
) -> Result<ToolResult, ToolError> {
    let mut files = Vec::new();
    let mut total = 0;
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

    let incomplete = walk.stopped().or(walk.unreadable().then_some(UNREADABLE));
    let mut output = match (files.is_empty(), incomplete) {
        (true, Some(_)) => "No files found so far".to_string(),
        (true, None) => "No files found".to_string(),
        (false, _) => files.join("\n"),
    };
    if total > files.len() {
        output.push_str(&format!("\n[truncated; omitted={}]", total - files.len()));
    }
    if let Some(reason) = incomplete {
        output.push_str(&format!("\n[{OPERATION} stopped early: {reason}]"));
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    use super::*;

    #[test]
    fn an_abandoned_listing_reads_no_further() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("own.rs"), "").unwrap();
        let context = ToolContext::default().within(root.path());
        let parameters = ListFilesParameters {
            path: ".".to_string(),
            recursive: true,
            pattern: None,
        };
        let listed = |abandoned: bool| {
            let mut walk = Walk::new(
                Instant::now() + context.search_timeout,
                Arc::new(AtomicBool::new(abandoned)),
                &context,
            );
            list(&mut walk, root.path(), &parameters)
                .unwrap()
                .output
                .unwrap()
        };

        assert_eq!(listed(false), "own.rs");
        assert_eq!(
            listed(true),
            "No files found so far\n[listing stopped early: abandoned]"
        );
    }
}
