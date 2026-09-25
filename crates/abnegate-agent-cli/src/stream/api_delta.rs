use serde::Deserialize;

/// The increment a `content_block_delta` event carries.
///
/// A variant may gain a field in a minor release, so a pattern outside this
/// crate ends in `..`:
///
/// ```compile_fail,E0638
/// use abnegate_agent_cli::ApiDelta;
///
/// fn text(delta: &ApiDelta) -> Option<&str> {
///     match delta {
///         ApiDelta::TextDelta { text } => Some(text),
///         _ => None,
///     }
/// }
/// # let _ = text;
/// ```
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "type")]
#[non_exhaustive]
pub enum ApiDelta {
    #[serde(rename = "text_delta")]
    #[non_exhaustive]
    TextDelta {
        #[serde(default)]
        text: String,
    },
    /// A fragment of a tool call's input, which is JSON only once every
    /// fragment has been joined.
    #[serde(rename = "input_json_delta")]
    #[non_exhaustive]
    InputJsonDelta {
        #[serde(default)]
        partial_json: String,
    },
    #[serde(other)]
    Other,
}

#[cfg(test)]
mod tests {
    use super::ApiDelta;

    #[test]
    fn an_unknown_delta_is_other() {
        let delta: ApiDelta =
            serde_json::from_str(r#"{"type":"thinking_delta","thinking":"hm"}"#).expect("a delta");
        assert_eq!(delta, ApiDelta::Other);
    }
}
