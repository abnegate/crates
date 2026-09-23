use serde::Serialize;

use crate::wire::message::Message;
use crate::wire::tool_choice::ToolChoice;
use crate::wire::tool_definition::ToolDefinition;

/// A chat completion request.
///
/// Borrowed because the agent loop already owns the conversation and the tool
/// definitions, and a request that took them by value would clone the whole
/// history once per turn.
#[derive(Debug, Clone, Serialize)]
pub struct ChatRequest<'a> {
    pub model: &'a str,
    pub messages: &'a [Message],
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<&'a [ToolDefinition]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<ToolChoice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop: Option<&'a [String]>,
}

#[cfg(test)]
mod tests {
    use super::ChatRequest;
    use crate::wire::message::Message;
    use crate::wire::tool_choice::ToolChoice;
    use crate::wire::tool_definition::ToolDefinition;

    #[test]
    fn a_request_carries_its_model_body_and_sampling() {
        let messages = [Message::user("Hello")];
        let request = ChatRequest {
            model: "gpt-4",
            messages: &messages,
            tools: None,
            tool_choice: None,
            temperature: Some(0.7),
            max_tokens: Some(1000),
            stream: Some(false),
            stop: None,
        };

        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("gpt-4"));
        assert!(json.contains("Hello"));
        assert!(json.contains("0.7"));
    }

    #[test]
    fn unset_options_are_omitted_rather_than_sent_as_null() {
        let messages = [Message::user("Hello")];
        let request = ChatRequest {
            model: "gpt-4",
            messages: &messages,
            tools: None,
            tool_choice: None,
            temperature: None,
            max_tokens: None,
            stream: None,
            stop: None,
        };

        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("gpt-4"));
        assert!(json.contains("Hello"));
        assert!(!json.contains("temperature"));
        assert!(!json.contains("max_tokens"));
        assert!(!json.contains("stream"));
    }

    #[test]
    fn tools_and_a_tool_choice_reach_the_body() {
        let tools = [ToolDefinition::function(
            "test_tool",
            "A test tool",
            serde_json::json!({"type": "object"}),
        )];
        let messages = [Message::user("Use the tool")];
        let request = ChatRequest {
            model: "gpt-4",
            messages: &messages,
            tools: Some(&tools),
            tool_choice: Some(ToolChoice::auto()),
            temperature: Some(0.5),
            max_tokens: Some(2048),
            stream: Some(false),
            stop: None,
        };

        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("test_tool"));
        assert!(json.contains("A test tool"));
        assert!(json.contains("auto"));
    }

    #[test]
    fn every_turn_of_the_conversation_reaches_the_body() {
        let messages = [
            Message::system("You are helpful"),
            Message::user("Hello"),
            Message::assistant("Hi there!"),
            Message::user("How are you?"),
        ];
        let request = ChatRequest {
            model: "gpt-4",
            messages: &messages,
            tools: None,
            tool_choice: None,
            temperature: None,
            max_tokens: None,
            stream: None,
            stop: None,
        };

        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("system"));
        assert!(json.contains("user"));
        assert!(json.contains("assistant"));
    }
}
