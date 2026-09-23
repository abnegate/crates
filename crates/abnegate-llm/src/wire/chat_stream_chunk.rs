use serde::Deserialize;

use crate::wire::stream_choice::StreamChoice;
use crate::wire::usage::Usage;

/// One streaming chunk.
///
/// Only `choices` carries content. The envelope fields are optional because
/// OpenAI-compatible providers do not all send them on every chunk, and a
/// chunk that fails to deserialise aborts the whole stream, losing the reply.
#[derive(Debug, Clone, Deserialize)]
#[non_exhaustive]
pub struct ChatStreamChunk {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub object: Option<String>,
    #[serde(default)]
    pub created: Option<i64>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub choices: Vec<StreamChoice>,
    #[serde(default)]
    pub usage: Option<Usage>,
}

#[cfg(test)]
mod tests {
    use super::ChatStreamChunk;
    use crate::wire::role::Role;

    #[test]
    fn a_chunk_carries_its_content_delta() {
        let json = r#"{
            "id": "chatcmpl-stream",
            "object": "chat.completion.chunk",
            "created": 1677652288,
            "model": "gpt-4",
            "choices": [{
                "index": 0,
                "delta": {
                    "content": "Hello"
                },
                "finish_reason": null
            }]
        }"#;

        let chunk: ChatStreamChunk = serde_json::from_str(json).unwrap();
        assert_eq!(chunk.id.as_deref(), Some("chatcmpl-stream"));
        assert_eq!(chunk.choices[0].delta.content, Some("Hello".to_string()));
        assert!(chunk.choices[0].finish_reason.is_none());
    }

    #[test]
    fn the_opening_chunk_carries_the_role() {
        let json = r#"{
            "id": "chatcmpl-stream",
            "object": "chat.completion.chunk",
            "created": 1677652288,
            "model": "gpt-4",
            "choices": [{
                "index": 0,
                "delta": {
                    "role": "assistant"
                },
                "finish_reason": null
            }]
        }"#;

        let chunk: ChatStreamChunk = serde_json::from_str(json).unwrap();
        assert_eq!(chunk.choices[0].delta.role, Some(Role::Assistant));
    }

    #[test]
    fn a_partial_tool_call_reads_without_its_closing_brace() {
        let json = r#"{
            "id": "chatcmpl-stream",
            "object": "chat.completion.chunk",
            "created": 1677652288,
            "model": "gpt-4",
            "choices": [{
                "index": 0,
                "delta": {
                    "tool_calls": [{
                        "index": 0,
                        "id": "call_123",
                        "type": "function",
                        "function": {
                            "name": "test_func",
                            "arguments": "{\"arg\":"
                        }
                    }]
                },
                "finish_reason": null
            }]
        }"#;

        let chunk: ChatStreamChunk = serde_json::from_str(json).unwrap();
        let tool_calls = chunk.choices[0].delta.tool_calls.as_ref().unwrap();
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].id, Some("call_123".to_string()));
        assert_eq!(
            tool_calls[0].function.as_ref().unwrap().name,
            Some("test_func".to_string())
        );
    }

    #[test]
    fn a_chunk_carries_generated_images() {
        let json = r#"{
            "choices": [{
                "index": 0,
                "delta": {
                    "images": [{
                        "image_url": {"url": "data:image/webp;base64,abc"},
                        "index": 0,
                        "type": "image_url"
                    }]
                },
                "finish_reason": null
            }]
        }"#;

        let chunk: ChatStreamChunk = serde_json::from_str(json).unwrap();
        assert_eq!(
            chunk.choices[0].delta.generated_images[0].image_url.url,
            "data:image/webp;base64,abc"
        );
    }

    #[test]
    fn a_null_image_list_does_not_abort_the_stream() {
        let json = r#"{
            "choices": [{
                "index": 0,
                "delta": {"images": null},
                "finish_reason": null
            }]
        }"#;

        let chunk: ChatStreamChunk = serde_json::from_str(json).unwrap();
        assert!(chunk.choices[0].delta.generated_images.is_empty());
    }

    #[test]
    fn a_chunk_reads_normalised_reasoning_content() {
        let chunk: ChatStreamChunk = serde_json::from_str(
            r#"{
            "choices": [{
                "index": 0,
                "delta": {
                    "reasoning_content": "step",
                    "thinking_blocks": [{"type":"thinking","thinking":"step"}]
                },
                "finish_reason": null
            }]
        }"#,
        )
        .unwrap();
        assert_eq!(
            chunk.choices[0].delta.reasoning_content.as_deref(),
            Some("step")
        );
        assert_eq!(chunk.choices[0].delta.thinking_blocks.len(), 1);
    }

    #[test]
    fn a_chunk_reads_the_thinking_alias() {
        let chunk: ChatStreamChunk = serde_json::from_str(
            r#"{
            "choices": [{
                "index": 0,
                "delta": {"thinking": "I should inspect the file."},
                "finish_reason": null
            }]
        }"#,
        )
        .unwrap();
        assert_eq!(
            chunk.choices[0].delta.reasoning_content.as_deref(),
            Some("I should inspect the file.")
        );
    }

    #[test]
    fn a_chunk_reads_the_reasoning_alias_and_null_thinking_blocks() {
        let chunk: ChatStreamChunk = serde_json::from_str(
            r#"{
            "choices": [{
                "index": 0,
                "delta": {"reasoning": "step", "thinking_blocks": null},
                "finish_reason": null
            }]
        }"#,
        )
        .unwrap();
        assert_eq!(
            chunk.choices[0].delta.reasoning_content.as_deref(),
            Some("step")
        );
        assert!(chunk.choices[0].delta.thinking_blocks.is_empty());
    }

    #[test]
    fn the_closing_chunk_carries_the_finish_reason() {
        let json = r#"{
            "id": "chatcmpl-stream",
            "object": "chat.completion.chunk",
            "created": 1677652288,
            "model": "gpt-4",
            "choices": [{
                "index": 0,
                "delta": {},
                "finish_reason": "stop"
            }]
        }"#;

        let chunk: ChatStreamChunk = serde_json::from_str(json).unwrap();
        assert_eq!(chunk.choices[0].finish_reason, Some("stop".to_string()));
    }
}
