use abnegate_llm::ToolCall;
use serde::Deserialize;
use serde::Serialize;

/// One tool call and what it returned.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallResult {
    pub call: ToolCall,
    /// The text the model was given back, as
    /// [`ToolResult::to_message`](crate::tools::ToolResult::to_message) put it.
    pub result: String,
    pub success: bool,
    #[serde(alias = "duration_ms")]
    pub duration_milliseconds: u64,
}

#[cfg(test)]
mod tests {
    use abnegate_llm::FunctionCall;

    use super::*;

    #[test]
    fn test_tool_call_result_failed() {
        let tool_call = ToolCall {
            id: "call_fail".to_string(),
            call_type: "function".to_string(),
            function: FunctionCall {
                name: "execute_command".to_string(),
                arguments: r#"{"cmd": "invalid"}"#.to_string(),
            },
        };

        let result = ToolCallResult {
            call: tool_call,
            result: "Error: command not found".to_string(),
            success: false,
            duration_milliseconds: 50,
        };

        assert!(!result.success);
        assert!(result.result.contains("Error"));
    }

    #[test]
    fn test_tool_call_result_serialization() {
        let tool_call = ToolCall {
            id: "call_test".to_string(),
            call_type: "function".to_string(),
            function: FunctionCall {
                name: "test".to_string(),
                arguments: "{}".to_string(),
            },
        };

        let result = ToolCallResult {
            call: tool_call,
            result: "success".to_string(),
            success: true,
            duration_milliseconds: 100,
        };

        let json = serde_json::to_string(&result).unwrap();
        let deserialized: ToolCallResult = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.call.id, "call_test");
        assert!(deserialized.success);
        assert_eq!(deserialized.duration_milliseconds, 100);
    }
}
