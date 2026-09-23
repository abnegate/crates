use serde::Deserialize;

use crate::parser::claude::CliUsage;
use crate::stream::api_content_block::ApiContentBlock;
use crate::stream::api_delta::ApiDelta;
use crate::stream::api_error::ApiError;
use crate::stream::api_message::ApiMessage;
use crate::stream::api_message_delta::ApiMessageDelta;

/// One event of the Messages API's streaming response.
///
/// Each block event carries the `index` of the block it belongs to, since a
/// message's blocks, a text block and a tool call say, can interleave.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "type")]
#[non_exhaustive]
pub enum ApiStreamEvent {
    #[serde(rename = "message_start")]
    MessageStart {
        #[serde(default)]
        message: Option<ApiMessage>,
    },
    #[serde(rename = "content_block_start")]
    ContentBlockStart {
        #[serde(default)]
        index: Option<usize>,
        #[serde(default)]
        content_block: Option<ApiContentBlock>,
    },
    #[serde(rename = "content_block_delta")]
    ContentBlockDelta {
        #[serde(default)]
        index: Option<usize>,
        #[serde(default)]
        delta: Option<ApiDelta>,
    },
    #[serde(rename = "content_block_stop")]
    ContentBlockStop {
        #[serde(default)]
        index: Option<usize>,
    },
    /// How the message ended, and its output token count.
    #[serde(rename = "message_delta")]
    MessageDelta {
        #[serde(default)]
        delta: Option<ApiMessageDelta>,
        #[serde(default)]
        usage: Option<CliUsage>,
    },
    #[serde(rename = "message_stop")]
    MessageStop {},
    /// A failure mid-stream, after which no more of the message comes.
    #[serde(rename = "error")]
    Error {
        #[serde(default)]
        error: Option<ApiError>,
    },
    #[serde(other)]
    Unknown,
}

#[cfg(test)]
mod tests {
    use super::ApiStreamEvent;
    use crate::parser::claude::CliUsage;
    use crate::stream::api_content_block::ApiContentBlock;
    use crate::stream::api_delta::ApiDelta;
    use crate::stream::api_error::ApiError;
    use crate::stream::api_message::ApiMessage;
    use crate::stream::api_message_delta::ApiMessageDelta;

    fn parse(line: &str) -> ApiStreamEvent {
        serde_json::from_str(line).expect("an event")
    }

    #[test]
    fn a_text_delta_is_read_with_its_block_index() {
        assert_eq!(
            parse(
                r#"{"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"hello"}}"#
            ),
            ApiStreamEvent::ContentBlockDelta {
                index: Some(1),
                delta: Some(ApiDelta::TextDelta {
                    text: "hello".to_string()
                })
            }
        );
    }

    #[test]
    fn an_input_json_delta_is_read() {
        assert_eq!(
            parse(
                r#"{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"key\":"}}"#
            ),
            ApiStreamEvent::ContentBlockDelta {
                index: Some(0),
                delta: Some(ApiDelta::InputJsonDelta {
                    partial_json: r#"{"key":"#.to_string()
                })
            }
        );
    }

    #[test]
    fn a_tool_use_block_start_is_read() {
        assert_eq!(
            parse(
                r#"{"type":"content_block_start","index":2,"content_block":{"type":"tool_use","id":"tu_1","name":"Bash"}}"#
            ),
            ApiStreamEvent::ContentBlockStart {
                index: Some(2),
                content_block: Some(ApiContentBlock::ToolUse {
                    id: "tu_1".to_string(),
                    name: "Bash".to_string()
                })
            }
        );
    }

    #[test]
    fn a_text_block_start_without_an_index_is_still_read() {
        assert_eq!(
            parse(r#"{"type":"content_block_start","content_block":{"type":"text","text":""}}"#),
            ApiStreamEvent::ContentBlockStart {
                index: None,
                content_block: Some(ApiContentBlock::Text {
                    text: String::new()
                })
            }
        );
    }

    #[test]
    fn a_message_start_keeps_its_identity_and_prompt_usage() {
        assert_eq!(
            parse(
                r#"{"type":"message_start","message":{"id":"msg_1","type":"message","role":"assistant","model":"claude-opus-4","content":[],"stop_reason":null,"usage":{"input_tokens":4,"cache_read_input_tokens":800,"output_tokens":1}}}"#
            ),
            ApiStreamEvent::MessageStart {
                message: Some(ApiMessage {
                    id: Some("msg_1".to_string()),
                    model: Some("claude-opus-4".to_string()),
                    stop_reason: None,
                    usage: Some(CliUsage {
                        input_tokens: Some(4),
                        output_tokens: Some(1),
                        cache_read_input_tokens: Some(800),
                        cache_creation_input_tokens: None,
                    }),
                }),
            }
        );
        assert_eq!(
            parse(r#"{"type":"message_start"}"#),
            ApiStreamEvent::MessageStart { message: None }
        );
    }

    #[test]
    fn a_message_delta_keeps_its_stop_reason_and_output_usage() {
        assert_eq!(
            parse(
                r#"{"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null},"usage":{"output_tokens":15}}"#
            ),
            ApiStreamEvent::MessageDelta {
                delta: Some(ApiMessageDelta {
                    stop_reason: Some("end_turn".to_string()),
                    stop_sequence: None,
                }),
                usage: Some(CliUsage {
                    output_tokens: Some(15),
                    ..CliUsage::default()
                }),
            }
        );
    }

    #[test]
    fn an_error_mid_stream_keeps_its_kind_and_message() {
        assert_eq!(
            parse(r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#),
            ApiStreamEvent::Error {
                error: Some(ApiError {
                    kind: Some("overloaded_error".to_string()),
                    message: Some("Overloaded".to_string()),
                }),
            }
        );
    }

    #[test]
    fn lifecycle_events_are_read() {
        assert_eq!(
            parse(r#"{"type":"content_block_stop","index":0}"#),
            ApiStreamEvent::ContentBlockStop { index: Some(0) }
        );
        assert_eq!(
            parse(r#"{"type":"message_stop"}"#),
            ApiStreamEvent::MessageStop {}
        );
    }

    #[test]
    fn an_unknown_event_is_kept_for_forward_compatibility() {
        for line in [
            r#"{"type":"some_future_event","data":"anything"}"#,
            r#"{"type":"ping"}"#,
        ] {
            assert_eq!(parse(line), ApiStreamEvent::Unknown, "{line}");
        }
    }
}
