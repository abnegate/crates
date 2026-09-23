use abnegate_secret::sanitize_owned;
use serde::{Deserialize, Serialize};

use super::text::{ERROR_PREFIX, MAX_TOOL_MESSAGE_CHARS, trim_middle};

/// What a tool call produced, success or failure, as the model will read it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub success: bool,
    /// The output of a successful call.
    pub output: Option<String>,
    /// The message of a failed call.
    pub error: Option<String>,
    /// Artifact URLs the loop should surface as generated images.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<String>,
}

impl ToolResult {
    /// Wrap successful tool output, [`sanitize`](super::sanitize)d on the way in.
    pub fn success(output: impl Into<String>) -> Self {
        Self {
            success: true,
            output: Some(sanitize_owned(output.into())),
            error: None,
            images: Vec::new(),
        }
    }

    /// Wrap a tool failure, [`sanitize`](super::sanitize)d on the way in.
    pub fn error(error: impl Into<String>) -> Self {
        Self {
            success: false,
            output: None,
            error: Some(sanitize_owned(error.into())),
            images: Vec::new(),
        }
    }

    pub fn with_images(mut self, images: Vec<String>) -> Self {
        self.images = images;
        self
    }

    /// The text the model is given for this result, capped at
    /// [`MAX_TOOL_MESSAGE_CHARS`].
    pub fn to_message(&self) -> String {
        let message = if self.success {
            self.output.clone().unwrap_or_default()
        } else {
            format!(
                "{ERROR_PREFIX}{}",
                self.error.as_deref().unwrap_or("Unknown error")
            )
        };
        trim_middle(&message, MAX_TOOL_MESSAGE_CHARS)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::MAX_TOOL_OUTPUT_CHARS;

    #[test]
    fn test_tool_result_success() {
        let result = ToolResult::success("Operation completed");
        assert!(result.success);
        assert_eq!(result.output, Some("Operation completed".to_string()));
        assert!(result.error.is_none());
        assert_eq!(result.to_message(), "Operation completed");
    }

    #[test]
    fn test_tool_result_error() {
        let result = ToolResult::error("Something went wrong");
        assert!(!result.success);
        assert!(result.output.is_none());
        assert_eq!(result.error, Some("Something went wrong".to_string()));
        assert_eq!(result.to_message(), "Error: Something went wrong");
    }

    #[test]
    fn success_redacts_a_credential_in_the_output() {
        let result = ToolResult::success("printenv\nGITHUB_TOKEN=ghp_0123456789abcdefghij\n");
        assert_eq!(
            result.output.as_deref(),
            Some("printenv\nGITHUB_TOKEN=[REDACTED]\n")
        );
        assert_eq!(result.to_message(), "printenv\nGITHUB_TOKEN=[REDACTED]\n");
    }

    #[test]
    fn error_strips_terminal_control_sequences() {
        let result = ToolResult::error("\u{1b}]0;stolen title\u{7}command not found\r\n");
        assert_eq!(result.error.as_deref(), Some("command not found\n"));
        assert_eq!(result.to_message(), "Error: command not found\n");
    }

    #[test]
    fn to_message_keeps_the_head_and_tail_of_huge_success_output() {
        let body = format!("HEAD_MARKER{}TAIL_MARKER", "x".repeat(20_000));
        let message = ToolResult::success(body).to_message();
        assert!(message.starts_with("HEAD_MARKER"), "{message}");
        assert!(message.ends_with("TAIL_MARKER"), "{message}");
        assert!(message.contains("characters trimmed"), "{message}");
        assert!(
            message.chars().count() <= MAX_TOOL_MESSAGE_CHARS,
            "{message}"
        );
    }

    #[test]
    fn to_message_caps_huge_error_on_character_boundary() {
        let result = ToolResult::error("é".repeat(20_000));
        let message = result.to_message();
        assert!(message.starts_with("Error: é"), "{message}");
        assert!(message.ends_with('é'), "{message}");
        assert!(message.contains("characters trimmed"), "{message}");
        assert!(!message.contains('\u{fffd}'), "{message}");
        assert!(
            message.chars().count() <= MAX_TOOL_MESSAGE_CHARS,
            "{message}"
        );
    }

    /// A tool that pages itself already returns the right amount; the
    /// transcript cap is there for one that does not. When the two budgets
    /// were equal, a full `read_file` page plus its pagination footer
    /// overflowed by the length of the footer and lost its middle here, so the
    /// model received the first and last halves of a page with the body gone.
    #[test]
    fn a_full_page_and_its_framing_are_not_cut_a_second_time() {
        let page = "p".repeat(MAX_TOOL_OUTPUT_CHARS);
        let framed =
            format!("{page}\n[truncated; total=99999 offset=0 next={MAX_TOOL_OUTPUT_CHARS}]");
        assert!(
            framed.chars().count() > MAX_TOOL_OUTPUT_CHARS,
            "the framing has to overflow the tool budget for this to be a test"
        );

        let message = ToolResult::success(framed.clone()).to_message();

        assert_eq!(message, framed, "a page that fits its budget was trimmed");
        assert!(!message.contains("characters trimmed"), "{message}");
    }

    #[test]
    fn test_tool_result_serialization() {
        let success = ToolResult::success("done");
        let json = serde_json::to_string(&success).unwrap();
        assert!(json.contains("\"success\":true"));
        assert!(json.contains("\"output\":\"done\""));

        let error = ToolResult::error("failed");
        let json = serde_json::to_string(&error).unwrap();
        assert!(json.contains("\"success\":false"));
        assert!(json.contains("\"error\":\"failed\""));
    }
}
