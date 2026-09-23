mod params;

use async_trait::async_trait;
use serde_json::{Value, json};
use std::io::Read;
use std::path::Path;

use crate::tools::beneath::{self, Access};
use crate::tools::{MAX_TOOL_OUTPUT_CHARS, Tool, ToolContext, ToolError, ToolResult};
use params::ReadFileParams;

/// A page of file text, the same budget every tool spends on output it pages
/// for itself.
pub(super) const FILE_PAGE_CHARS: usize = MAX_TOOL_OUTPUT_CHARS;

/// Read a file's contents
pub struct ReadFileTool;

#[async_trait]
impl Tool for ReadFileTool {
    fn name(&self) -> &str {
        "read_file"
    }

    fn description(&self) -> &str {
        "Read the contents of a file. Optionally specify start_line and end_line for a line range, and offset and limit to page by character."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file to read (relative to working directory)"
                },
                "start_line": {
                    "type": "integer",
                    "description": "Start line (1-indexed, optional)"
                },
                "end_line": {
                    "type": "integer",
                    "description": "End line (1-indexed, optional)"
                },
                "offset": {
                    "type": "integer",
                    "description": "Unicode character offset to start from (default 0)"
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum characters to return (default 8000, max 8000)"
                }
            },
            "required": ["path"]
        })
    }

    async fn execute(&self, params: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let params: ReadFileParams = serde_json::from_value(params)
            .map_err(|error| ToolError::InvalidParams(error.to_string()))?;

        let mut file = beneath::open(context, Path::new(&params.path), Access::Read)?;

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

        let selected = if params.start_line.is_some() || params.end_line.is_some() {
            let lines: Vec<&str> = content.lines().collect();
            let start = params.start_line.unwrap_or(1).saturating_sub(1);
            let end = params.end_line.unwrap_or(lines.len()).min(lines.len());

            lines[start..end].join("\n")
        } else {
            content
        };

        let offset = params.offset.unwrap_or(0);
        let limit = params.limit.unwrap_or(FILE_PAGE_CHARS);
        let (page, total, next) = page_text(&selected, offset, limit)?;
        Ok(ToolResult::success(format_file_page(
            page, total, offset, next,
        )))
    }
}

pub(super) fn page_text(
    content: &str,
    offset: usize,
    limit: usize,
) -> Result<(String, usize, Option<usize>), ToolError> {
    let total = content.chars().count();
    if limit == 0 || offset > total {
        return Err(ToolError::InvalidParams(
            "File page offset or length is invalid.".into(),
        ));
    }
    let count = limit.min(FILE_PAGE_CHARS).min(total.saturating_sub(offset));
    let page: String = content.chars().skip(offset).take(count).collect();
    let end = offset + count;
    Ok((page, total, (end < total).then_some(end)))
}

fn format_file_page(page: String, total: usize, offset: usize, next: Option<usize>) -> String {
    match next {
        Some(next) => format!("{page}\n[truncated; total={total} offset={offset} next={next}]"),
        None => page,
    }
}
