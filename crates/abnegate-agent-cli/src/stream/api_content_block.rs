use serde::Deserialize;

/// The block a `content_block_start` event opens.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "type")]
#[non_exhaustive]
pub enum ApiContentBlock {
    #[serde(rename = "text")]
    #[non_exhaustive]
    Text {
        #[serde(default)]
        text: String,
    },
    #[serde(rename = "tool_use")]
    #[non_exhaustive]
    ToolUse {
        #[serde(default)]
        id: String,
        #[serde(default)]
        name: String,
    },
    #[serde(other)]
    Other,
}

#[cfg(test)]
mod tests {
    use super::ApiContentBlock;

    #[test]
    fn missing_fields_default_to_empty() {
        let block: ApiContentBlock =
            serde_json::from_str(r#"{"type":"tool_use"}"#).expect("a block");
        assert_eq!(
            block,
            ApiContentBlock::ToolUse {
                id: String::new(),
                name: String::new()
            }
        );
    }

    #[test]
    fn an_unknown_block_is_other() {
        let block: ApiContentBlock =
            serde_json::from_str(r#"{"type":"thinking","thinking":"hmm"}"#).expect("a block");
        assert_eq!(block, ApiContentBlock::Other);
    }
}
