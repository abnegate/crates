use serde::{Deserialize, Serialize};

use crate::wire::function_call::FunctionCall;

/// A tool call request from the model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub call_type: String,
    pub function: FunctionCall,
}

#[cfg(test)]
mod tests {
    use super::ToolCall;
    use crate::wire::function_call::FunctionCall;

    #[test]
    fn a_tool_call_round_trips() {
        let call = ToolCall {
            id: "call_test123".to_string(),
            call_type: "function".to_string(),
            function: FunctionCall {
                name: "execute_command".to_string(),
                arguments: r#"{"command": "ls -la"}"#.to_string(),
            },
        };

        let json = serde_json::to_string(&call).unwrap();
        let deserialized: ToolCall = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.id, "call_test123");
        assert_eq!(deserialized.call_type, "function");
        assert_eq!(deserialized.function.name, "execute_command");
        assert_eq!(deserialized.function.arguments, r#"{"command": "ls -la"}"#);
    }

    #[test]
    fn a_tool_call_with_no_arguments_round_trips() {
        let call = ToolCall {
            id: "call_empty".to_string(),
            call_type: "function".to_string(),
            function: FunctionCall {
                name: "no_args_function".to_string(),
                arguments: "{}".to_string(),
            },
        };

        let json = serde_json::to_string(&call).unwrap();
        let deserialized: ToolCall = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.function.arguments, "{}");
    }

    #[test]
    fn a_tool_call_with_complex_arguments_round_trips() {
        let arguments = r#"{"nested": {"key": "value"}, "array": [1, 2, 3], "null_field": null}"#;
        let call = ToolCall {
            id: "call_complex".to_string(),
            call_type: "function".to_string(),
            function: FunctionCall {
                name: "complex_function".to_string(),
                arguments: arguments.to_string(),
            },
        };

        let json = serde_json::to_string(&call).unwrap();
        let deserialized: ToolCall = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.function.arguments, arguments);
    }

    #[test]
    fn serialisation_never_emits_concatenated_json() {
        let call = ToolCall {
            id: "call_1".to_string(),
            call_type: "function".to_string(),
            function: FunctionCall {
                name: "search_knowledge".to_string(),
                arguments: r#"{"query":"documents reminders console"}{"limit":5}"#.to_string(),
            },
        };
        let json = serde_json::to_value(&call).unwrap();
        let arguments = json["function"]["arguments"].as_str().unwrap();
        assert_eq!(arguments, r#"{"query":"documents reminders console"}"#);
        assert!(serde_json::from_str::<serde_json::Value>(arguments).is_ok());
    }
}
