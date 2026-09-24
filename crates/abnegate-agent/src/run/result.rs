use std::time::Duration;

use abnegate_llm::ToolCall;
use serde::Deserialize;
use serde::Serialize;

/// One tool call and what it returned.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallResult {
    pub call: ToolCall,
    /// The text the model was given back, as
    /// [`ToolResult::to_message`](crate::tool::ToolResult::to_message) put it.
    pub result: String,
    pub success: bool,
    /// How long the call took, the wait for its approval included. Saved as
    /// whole milliseconds.
    #[serde(
        rename = "duration_milliseconds",
        alias = "duration_ms",
        with = "super::milliseconds"
    )]
    pub duration: Duration,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn test_tool_call_result_failed() {
        let tool_call = ToolCall::function("call_fail", "execute_command", r#"{"cmd": "invalid"}"#);

        let result = ToolCallResult {
            call: tool_call,
            result: "Error: command not found".to_string(),
            success: false,
            duration: Duration::from_millis(50),
        };

        assert!(!result.success);
        assert!(result.result.contains("Error"));
    }

    #[test]
    fn test_tool_call_result_serialization() {
        let tool_call = ToolCall::function("call_test", "test", "{}");

        let result = ToolCallResult {
            call: tool_call,
            result: "success".to_string(),
            success: true,
            duration: Duration::from_millis(100),
        };

        let json = serde_json::to_string(&result).unwrap();
        let deserialized: ToolCallResult = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.call.id, "call_test");
        assert!(deserialized.success);
        assert_eq!(deserialized.duration, Duration::from_millis(100));
    }

    /// The duration became a [`Duration`]; a saved session still holds it as
    /// whole milliseconds under the name it was written with.
    #[test]
    fn the_duration_is_saved_as_whole_milliseconds_under_its_old_name() {
        let result = ToolCallResult {
            call: ToolCall::function("call_test", "test", "{}"),
            result: "success".to_string(),
            success: true,
            duration: Duration::from_micros(1_500_999),
        };

        let saved = serde_json::to_value(&result).unwrap();

        assert_eq!(saved["duration_milliseconds"], json!(1_500));
        assert!(saved.get("duration").is_none(), "{saved}");
    }

    #[test]
    fn a_result_saved_under_either_earlier_name_still_reads() {
        for name in ["duration_milliseconds", "duration_ms"] {
            let saved = json!({
                "call": {"id": "call_1", "type": "function", "function": {"name": "test", "arguments": "{}"}},
                "result": "ran",
                "success": true,
                name: 250
            });

            let result: ToolCallResult = serde_json::from_value(saved).unwrap();

            assert_eq!(result.duration, Duration::from_millis(250), "{name}");
        }
    }
}
