use serde::Deserialize;

use crate::stream::api_content_block::ApiContentBlock;
use crate::stream::api_delta::ApiDelta;

/// One event of the Messages API's streaming response.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "type")]
#[non_exhaustive]
pub enum ApiStreamEvent {
    #[serde(rename = "message_start")]
    MessageStart {},
    #[serde(rename = "content_block_start")]
    ContentBlockStart {
        #[serde(default)]
        content_block: Option<ApiContentBlock>,
    },
    #[serde(rename = "content_block_delta")]
    ContentBlockDelta {
        #[serde(default)]
        delta: Option<ApiDelta>,
    },
    #[serde(rename = "content_block_stop")]
    ContentBlockStop {},
    #[serde(rename = "message_delta")]
    MessageDelta {},
    #[serde(rename = "message_stop")]
    MessageStop {},
    #[serde(other)]
    Unknown,
}

#[cfg(test)]
mod tests {
    use super::ApiStreamEvent;
    use crate::stream::api_content_block::ApiContentBlock;
    use crate::stream::api_delta::ApiDelta;

    fn parse(line: &str) -> ApiStreamEvent {
        serde_json::from_str(line).expect("an event")
    }

    #[test]
    fn a_text_delta_is_read() {
        assert_eq!(
            parse(r#"{"type":"content_block_delta","delta":{"type":"text_delta","text":"hello"}}"#),
            ApiStreamEvent::ContentBlockDelta {
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
                r#"{"type":"content_block_delta","delta":{"type":"input_json_delta","partial_json":"{\"key\":"}}"#
            ),
            ApiStreamEvent::ContentBlockDelta {
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
                r#"{"type":"content_block_start","content_block":{"type":"tool_use","id":"tu_1","name":"Bash"}}"#
            ),
            ApiStreamEvent::ContentBlockStart {
                content_block: Some(ApiContentBlock::ToolUse {
                    id: "tu_1".to_string(),
                    name: "Bash".to_string()
                })
            }
        );
    }

    #[test]
    fn a_text_block_start_is_read() {
        assert_eq!(
            parse(r#"{"type":"content_block_start","content_block":{"type":"text","text":""}}"#),
            ApiStreamEvent::ContentBlockStart {
                content_block: Some(ApiContentBlock::Text {
                    text: String::new()
                })
            }
        );
    }

    #[test]
    fn lifecycle_events_are_read_with_their_payloads_ignored() {
        assert_eq!(
            parse(r#"{"type":"message_start"}"#),
            ApiStreamEvent::MessageStart {}
        );
        assert_eq!(
            parse(
                r#"{"type":"message_start","message":{"id":"msg_1","usage":{"input_tokens":4}}}"#
            ),
            ApiStreamEvent::MessageStart {}
        );
        assert_eq!(
            parse(r#"{"type":"content_block_stop","index":0}"#),
            ApiStreamEvent::ContentBlockStop {}
        );
        assert_eq!(
            parse(r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"}}"#),
            ApiStreamEvent::MessageDelta {}
        );
        assert_eq!(
            parse(r#"{"type":"message_stop"}"#),
            ApiStreamEvent::MessageStop {}
        );
    }

    #[test]
    fn an_unknown_event_is_kept_for_forward_compatibility() {
        assert_eq!(
            parse(r#"{"type":"some_future_event","data":"anything"}"#),
            ApiStreamEvent::Unknown
        );
    }
}
