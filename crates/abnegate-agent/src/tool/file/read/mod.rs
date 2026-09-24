mod parameters;

use std::path::PathBuf;

use async_trait::async_trait;
use parameters::ReadFileParameters;
use serde_json::Value;
use serde_json::json;

use super::blocking;
use super::read_text;
use crate::tool::MAX_TOOL_OUTPUT_CHARACTERS;
use crate::tool::Tool;
use crate::tool::ToolContext;
use crate::tool::ToolError;
use crate::tool::ToolResult;

/// A page of file text, the same budget every tool spends on output it pages
/// for itself.
pub(super) const FILE_PAGE_CHARACTERS: usize = MAX_TOOL_OUTPUT_CHARACTERS;

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

    async fn execute(
        &self,
        parameters: Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let parameters: ReadFileParameters = serde_json::from_value(parameters)
            .map_err(|error| ToolError::InvalidParameters(error.to_string()))?;

        let path = PathBuf::from(&parameters.path);
        let context = context.clone();
        let content = blocking(move || read_text(&context, &path)).await?;

        let selected = if parameters.start_line.is_some() || parameters.end_line.is_some() {
            select_lines(&content, parameters.start_line, parameters.end_line)?
        } else {
            content
        };

        let offset = parameters.offset.unwrap_or(0);
        let limit = parameters.limit.unwrap_or(FILE_PAGE_CHARACTERS);
        let (page, total, next) = page_text(&selected, offset, limit)?;
        Ok(ToolResult::success(format_file_page(
            page, total, offset, next,
        )))
    }
}

/// Lines `start_line` to `end_line` of `content`, both 1-indexed and
/// inclusive, with an end past the last line read as the last line.
pub(super) fn select_lines(
    content: &str,
    start_line: Option<usize>,
    end_line: Option<usize>,
) -> Result<String, ToolError> {
    let lines: Vec<&str> = content.lines().collect();
    let first = start_line.unwrap_or(1).max(1);
    let last = end_line.unwrap_or(lines.len());
    let start = first - 1;
    let end = last.min(lines.len());
    if last < first.min(lines.len()) || start > end {
        return Err(ToolError::InvalidParameters(format!(
            "Lines {first} to {last} are not a range in a file of {} lines.",
            lines.len()
        )));
    }
    Ok(lines[start..end].join("\n"))
}

pub(super) fn page_text(
    content: &str,
    offset: usize,
    limit: usize,
) -> Result<(String, usize, Option<usize>), ToolError> {
    let total = content.chars().count();
    if limit == 0 || offset > total {
        return Err(ToolError::InvalidParameters(
            "File page offset or length is invalid.".into(),
        ));
    }
    let count = limit
        .min(FILE_PAGE_CHARACTERS)
        .min(total.saturating_sub(offset));
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
