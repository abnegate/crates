use serde::Deserialize;

/// The increment a `content_block_delta` event carries.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "type")]
pub enum ApiDelta {
    #[serde(rename = "text_delta")]
    TextDelta {
        #[serde(default)]
        text: String,
    },
    /// A fragment of a tool call's input, which is JSON only once every
    /// fragment has been joined.
    #[serde(rename = "input_json_delta")]
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
