use serde::Deserialize;

/// The block a `content_block_start` event opens.
///
/// A variant may gain a field in a minor release, so a pattern outside this
/// crate ends in `..`:
///
/// ```compile_fail,E0638
/// use abnegate_agent_cli::ApiContentBlock;
///
/// fn text(block: &ApiContentBlock) -> Option<&str> {
///     match block {
///         ApiContentBlock::Text { text } => Some(text),
///         _ => None,
///     }
/// }
/// # let _ = text;
/// ```
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "type")]
#[non_exhaustive]
pub enum ApiContentBlock {
    /// A block of prose, whose text the deltas after it stream.
    #[serde(rename = "text")]
    #[non_exhaustive]
    Text {
        /// Any text the block opens with, usually none.
        #[serde(default)]
        text: String,
    },
    /// A tool call, whose input the deltas after it stream.
    #[serde(rename = "tool_use")]
    #[non_exhaustive]
    ToolUse {
        /// The call's identifier, which its result names.
        #[serde(default)]
        id: String,
        /// The tool called.
        #[serde(default)]
        name: String,
    },
    /// Any other kind of block, such as thinking, which is ignored.
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
