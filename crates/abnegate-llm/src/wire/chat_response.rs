use serde::Deserialize;

use crate::wire::choice::Choice;
use crate::wire::usage::Usage;

/// A chat completion response.
///
/// Only `choices` carries the answer. The envelope fields default when an
/// OpenAI-compatible server leaves them out, rather than failing a reply that
/// is otherwise complete.
#[derive(Debug, Clone, Deserialize)]
#[non_exhaustive]
pub struct ChatResponse {
    /// The completion's identifier, or empty.
    #[serde(default)]
    pub id: String,
    /// The object type, `chat.completion`, or empty.
    #[serde(default)]
    pub object: String,
    /// When it was created, in Unix seconds, or 0.
    #[serde(default)]
    pub created: i64,
    /// The model that answered, or empty.
    #[serde(default)]
    pub model: String,
    /// The answers; one unless more were asked for.
    pub choices: Vec<Choice>,
    /// The tokens it took, when the server said.
    pub usage: Option<Usage>,
}

impl ChatResponse {
    const OBJECT: &str = "chat.completion";

    /// A completion answering with `choices`, of the object type
    /// `chat.completion`, with the rest of its envelope empty.
    pub fn new(choices: Vec<Choice>) -> Self {
        Self {
            id: String::new(),
            object: Self::OBJECT.to_string(),
            created: 0,
            model: String::new(),
            choices,
            usage: None,
        }
    }

    /// The same response, identified as `id`.
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = id.into();
        self
    }

    /// The same response, created at `created` Unix seconds.
    pub fn with_created(mut self, created: i64) -> Self {
        self.created = created;
        self
    }

    /// The same response, answered by `model`.
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    /// The same response, having taken `usage`.
    pub fn with_usage(mut self, usage: Usage) -> Self {
        self.usage = Some(usage);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::ChatResponse;
    use crate::wire::choice::Choice;
    use crate::wire::message::Message;
    use crate::wire::usage::Usage;

    #[test]
    fn a_built_response_reads_like_one_an_endpoint_sent() {
        let response = ChatResponse::new(vec![
            Choice::new(0, Message::assistant("It is empty.")).with_finish_reason("stop"),
        ])
        .with_id("claude-6f1")
        .with_created(1_677_652_288)
        .with_model("sonnet")
        .with_usage(Usage::new(9, 24));

        assert_eq!(response.id, "claude-6f1");
        assert_eq!(response.object, "chat.completion");
        assert_eq!(response.created, 1_677_652_288);
        assert_eq!(response.model, "sonnet");
        assert_eq!(
            response.choices[0].message.content.as_deref(),
            Some("It is empty.")
        );
        assert_eq!(response.choices[0].finish_reason.as_deref(), Some("stop"));
        assert_eq!(response.usage, Some(Usage::new(9, 24)));
    }

    #[test]
    fn a_new_response_has_only_its_object_type() {
        let response = ChatResponse::new(Vec::new());

        assert!(response.id.is_empty());
        assert_eq!(response.object, "chat.completion");
        assert_eq!(response.created, 0);
        assert!(response.model.is_empty());
        assert!(response.usage.is_none());
    }

    #[test]
    fn a_response_carries_its_choice_and_usage() {
        let json = r#"{
            "id": "chatcmpl-123",
            "object": "chat.completion",
            "created": 1677652288,
            "model": "gpt-4",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "Hello!"
                },
                "finish_reason": "stop"
            }],
            "usage": {
                "prompt_tokens": 10,
                "completion_tokens": 20,
                "total_tokens": 30
            }
        }"#;

        let response: ChatResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.id, "chatcmpl-123");
        assert_eq!(response.model, "gpt-4");
        assert_eq!(response.choices.len(), 1);
        assert_eq!(
            response.choices[0].message.content,
            Some("Hello!".to_string())
        );
        assert_eq!(response.usage.unwrap().total_tokens, 30);
    }

    #[test]
    fn a_response_without_its_envelope_still_reads() {
        let json = r#"{
            "choices": [{
                "message": { "role": "assistant", "content": "Hello!" },
                "finish_reason": "stop"
            }],
            "usage": { "prompt_tokens": 3 }
        }"#;

        let response: ChatResponse = serde_json::from_str(json).unwrap();
        assert!(response.id.is_empty());
        assert_eq!(response.created, 0);
        assert_eq!(response.choices[0].index, 0);
        assert_eq!(response.usage.unwrap().prompt_tokens, 3);
    }

    #[test]
    fn a_response_without_usage_still_reads() {
        let json = r#"{
            "id": "chatcmpl-456",
            "object": "chat.completion",
            "created": 1677652288,
            "model": "gpt-4",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "Response"
                },
                "finish_reason": "stop"
            }]
        }"#;

        let response: ChatResponse = serde_json::from_str(json).unwrap();
        assert!(response.usage.is_none());
    }

    #[test]
    fn tool_calls_reach_the_caller() {
        let json = r#"{
            "id": "chatcmpl-789",
            "object": "chat.completion",
            "created": 1677652288,
            "model": "gpt-4",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "call_abc",
                        "type": "function",
                        "function": {
                            "name": "get_weather",
                            "arguments": "{\"location\": \"NYC\"}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        }"#;

        let response: ChatResponse = serde_json::from_str(json).unwrap();
        assert!(response.choices[0].message.content.is_none());
        let tool_calls = response.choices[0].message.tool_calls.as_ref().unwrap();
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].function.name, "get_weather");
    }

    #[test]
    fn every_choice_keeps_its_own_index() {
        let json = r#"{
            "id": "chatcmpl-multi",
            "object": "chat.completion",
            "created": 1677652288,
            "model": "gpt-4",
            "choices": [
                {
                    "index": 0,
                    "message": {"role": "assistant", "content": "Response 1"},
                    "finish_reason": "stop"
                },
                {
                    "index": 1,
                    "message": {"role": "assistant", "content": "Response 2"},
                    "finish_reason": "stop"
                }
            ]
        }"#;

        let response: ChatResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.choices.len(), 2);
        assert_eq!(response.choices[0].index, 0);
        assert_eq!(response.choices[1].index, 1);
    }

    #[test]
    fn generated_images_reach_the_caller() {
        let json = r#"{
            "id": "chatcmpl-image",
            "object": "chat.completion",
            "created": 1677652288,
            "model": "gemini-image",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "Here is your image.",
                    "images": [{
                        "image_url": {
                            "url": "data:image/png;base64,abc",
                            "detail": "auto"
                        },
                        "index": 0,
                        "type": "image_url"
                    }]
                },
                "finish_reason": "stop"
            }]
        }"#;

        let response: ChatResponse = serde_json::from_str(json).unwrap();
        assert_eq!(
            response.choices[0].message.generated_images[0]
                .image_url
                .url,
            "data:image/png;base64,abc"
        );
    }
}
