use std::time::Duration;

use abnegate_llm::ToolCall;
use serde::Deserialize;
use serde::Serialize;

/// One tool call and what it returned.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ToolCallResult {
    /// The call as it ran, under the id the run gave it.
    pub call: ToolCall,
    /// The text the model was given back, as
    /// [`ToolResult::to_message`](crate::tool::ToolResult::to_message) put it.
    pub result: String,
    /// Whether the tool reported success.
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

impl ToolCallResult {
    /// `call`, which returned `result` after `duration`, and succeeded when
    /// `success` is set.
    pub fn new(
        call: ToolCall,
        result: impl Into<String>,
        success: bool,
        duration: Duration,
    ) -> Self {
        Self {
            call,
            result: result.into(),
            success,
            duration,
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_result_carries_what_it_was_given() {
        let call = ToolCall::function("call_fail", "execute_command", r#"{"cmd": "invalid"}"#);

        let result = ToolCallResult::new(
            call,
            "Error: command not found",
            false,
            Duration::from_millis(50),
        );

        assert_eq!(result.call.id, "call_fail");
        assert_eq!(result.result, "Error: command not found");
        assert!(!result.success);
        assert_eq!(result.duration, Duration::from_millis(50));
    }

    #[test]
    fn test_tool_call_result_serialization() {
        let result = ToolCallResult::new(
            ToolCall::function("call_test", "test", "{}"),
            "success",
            true,
            Duration::from_millis(100),
        );

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
        let result = ToolCallResult::new(
            ToolCall::function("call_test", "test", "{}"),
            "success",
            true,
            Duration::from_micros(1_500_999),
        );

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
