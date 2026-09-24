use serde::{Deserialize, Serialize};

use crate::wire::function_call::FunctionCall;

const FUNCTION_TYPE: &str = "function";

/// A tool call request from the model.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub call_type: String,
    pub function: FunctionCall,
}

impl ToolCall {
    /// The call `id` to the function `name` with `arguments`, a JSON object as
    /// a string.
    pub fn function(
        id: impl Into<String>,
        name: impl Into<String>,
        arguments: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            call_type: FUNCTION_TYPE.to_string(),
            function: FunctionCall::new(name, arguments),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ToolCall;

    #[test]
    fn a_tool_call_round_trips() {
        let call = ToolCall::function(
            "call_test123",
            "execute_command",
            r#"{"command": "ls -la"}"#,
        );

        let json = serde_json::to_string(&call).unwrap();
        let deserialized: ToolCall = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.id, "call_test123");
        assert_eq!(deserialized.call_type, "function");
        assert_eq!(deserialized.function.name, "execute_command");
        assert_eq!(deserialized.function.arguments, r#"{"command": "ls -la"}"#);
    }

    #[test]
    fn a_tool_call_with_no_arguments_round_trips() {
        let call = ToolCall::function("call_empty", "no_args_function", "{}");

        let json = serde_json::to_string(&call).unwrap();
        let deserialized: ToolCall = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.function.arguments, "{}");
    }

    #[test]
    fn a_tool_call_with_complex_arguments_round_trips() {
        let arguments = r#"{"nested": {"key": "value"}, "array": [1, 2, 3], "null_field": null}"#;
        let call = ToolCall::function("call_complex", "complex_function", arguments);

        let json = serde_json::to_string(&call).unwrap();
        let deserialized: ToolCall = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.function.arguments, arguments);
    }

    #[test]
    fn serialisation_never_emits_concatenated_json() {
        let call = ToolCall::function(
            "call_1",
            "search_knowledge",
            r#"{"query":"documents reminders console"}{"limit":5}"#,
        );
        let json = serde_json::to_value(&call).unwrap();
        let arguments = json["function"]["arguments"].as_str().unwrap();
        assert_eq!(arguments, r#"{"query":"documents reminders console"}"#);
        assert!(serde_json::from_str::<serde_json::Value>(arguments).is_ok());
    }
}
