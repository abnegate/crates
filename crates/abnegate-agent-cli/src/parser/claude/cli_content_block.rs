use serde::Deserialize;

/// One block of an `assistant` event's `message.content`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "type")]
#[non_exhaustive]
pub enum CliContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        #[serde(default)]
        input: serde_json::Value,
    },
    #[serde(other)]
    Other,
}

#[cfg(test)]
mod tests {
    use super::CliContentBlock;

    #[test]
    fn a_text_block_is_read() {
        let block: CliContentBlock =
            serde_json::from_str(r#"{"type":"text","text":"hello"}"#).expect("a text block");
        assert_eq!(
            block,
            CliContentBlock::Text {
                text: "hello".to_string()
            }
        );
    }

    #[test]
    fn a_tool_use_block_is_read_with_or_without_its_input() {
        let block: CliContentBlock =
            serde_json::from_str(r#"{"type":"tool_use","id":"tool-abc","name":"Grep"}"#)
                .expect("a tool use block");
        let CliContentBlock::ToolUse { id, name, input } = block else {
            panic!("expected a tool use, got {block:?}");
        };
        assert_eq!(id, "tool-abc");
        assert_eq!(name, "Grep");
        assert!(input.is_null());

        let block: CliContentBlock = serde_json::from_str(
            r#"{"type":"tool_use","id":"toolu_01","name":"Read","input":{"file_path":"/w/a.rs"}}"#,
        )
        .expect("a tool use block");
        let CliContentBlock::ToolUse { input, .. } = block else {
            panic!("expected a tool use, got {block:?}");
        };
        assert_eq!(input["file_path"], "/w/a.rs");
    }

    #[test]
    fn a_tool_use_with_an_empty_name_still_parses() {
        let block: CliContentBlock =
            serde_json::from_str(r#"{"type":"tool_use","id":"tu_empty","name":""}"#)
                .expect("a tool use block");
        assert!(
            matches!(block, CliContentBlock::ToolUse { ref id, ref name, .. } if id == "tu_empty" && name.is_empty())
        );
    }

    #[test]
    fn every_other_block_type_is_other() {
        for kind in ["thinking", "tool_result", "image", "some_future_type"] {
            let json = format!(r#"{{"type":"{kind}","data":"whatever"}}"#);
            let block: CliContentBlock = serde_json::from_str(&json).expect("a block");
            assert_eq!(block, CliContentBlock::Other, "{kind}");
        }
    }

    #[test]
    fn empty_and_escaped_text_survives() {
        let block: CliContentBlock =
            serde_json::from_str(r#"{"type":"text","text":""}"#).expect("a text block");
        assert!(matches!(block, CliContentBlock::Text { ref text } if text.is_empty()));

        let block: CliContentBlock =
            serde_json::from_str(r#"{"type":"text","text":"line1\nline2\ttab\"quote\\"}"#)
                .expect("a text block");
        let CliContentBlock::Text { text } = block else {
            panic!("expected text, got {block:?}");
        };
        assert!(text.contains('\n'));
        assert!(text.contains('\t'));
        assert!(text.contains('"'));
        assert!(text.contains('\\'));
    }
}
