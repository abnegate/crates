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
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub object: String,
    #[serde(default)]
    pub created: i64,
    #[serde(default)]
    pub model: String,
    pub choices: Vec<Choice>,
    pub usage: Option<Usage>,
}

#[cfg(test)]
mod tests {
    use super::ChatResponse;

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
