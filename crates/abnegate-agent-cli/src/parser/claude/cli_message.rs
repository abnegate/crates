use serde::Deserialize;

use crate::parser::claude::cli_content_block::CliContentBlock;
use crate::parser::claude::cli_usage::CliUsage;

/// The `message` an `assistant` event carries.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct CliMessage {
    #[serde(default)]
    pub content: Vec<CliContentBlock>,
    #[serde(default)]
    pub usage: Option<CliUsage>,
}

#[cfg(test)]
mod tests {
    use super::CliMessage;
    use crate::parser::claude::cli_content_block::CliContentBlock;

    #[test]
    fn missing_or_empty_content_is_an_empty_list() {
        for json in [r#"{}"#, r#"{"content":[]}"#] {
            let message: CliMessage = serde_json::from_str(json).expect("a message");
            assert!(message.content.is_empty(), "{json}");
            assert!(message.usage.is_none(), "{json}");
        }
    }

    #[test]
    fn mixed_content_keeps_its_order() {
        let message: CliMessage = serde_json::from_str(
            r#"{"content":[
                {"type":"text","text":"Analyzing..."},
                {"type":"tool_use","id":"t1","name":"Bash"},
                {"type":"thinking","thinking":"hmm"},
                {"type":"text","text":"Done."}
            ]}"#,
        )
        .expect("a message");

        assert_eq!(message.content.len(), 4);
        assert!(
            matches!(&message.content[0], CliContentBlock::Text { text } if text == "Analyzing...")
        );
        assert!(
            matches!(&message.content[1], CliContentBlock::ToolUse { name, .. } if name == "Bash")
        );
        assert_eq!(message.content[2], CliContentBlock::Other);
        assert!(matches!(&message.content[3], CliContentBlock::Text { text } if text == "Done."));
    }

    #[test]
    fn a_single_text_block_and_its_usage_are_read() {
        let message: CliMessage = serde_json::from_str(
            r#"{"content":[{"type":"text","text":"only block"}],"usage":{"input_tokens":4,"output_tokens":6}}"#,
        )
        .expect("a message");

        assert_eq!(
            message.content,
            [CliContentBlock::Text {
                text: "only block".to_string()
            }]
        );
        assert_eq!(message.usage.and_then(|usage| usage.output_tokens), Some(6));
    }

    #[test]
    fn debug_names_the_type() {
        assert!(format!("{:?}", CliMessage::default()).contains("CliMessage"));
    }
}
