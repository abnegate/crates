use serde::Deserialize;

use crate::wire::stream_function_call::StreamFunctionCall;

/// A tool call arriving one fragment at a time.
///
/// A provider streaming a single call may leave out `index`; it reads as the
/// first call rather than failing the chunk.
#[derive(Debug, Clone, Deserialize)]
#[non_exhaustive]
pub struct StreamToolCall {
    #[serde(default)]
    pub index: u32,
    pub id: Option<String>,
    #[serde(rename = "type")]
    pub call_type: Option<String>,
    pub function: Option<StreamFunctionCall>,
}

#[cfg(test)]
mod tests {
    use super::StreamToolCall;

    #[test]
    fn a_fragment_without_an_index_is_the_first_call() {
        let call: StreamToolCall =
            serde_json::from_str(r#"{"function":{"arguments":"{\"path\""}}"#).unwrap();

        assert_eq!(call.index, 0);
        assert!(call.id.is_none());
    }
}
