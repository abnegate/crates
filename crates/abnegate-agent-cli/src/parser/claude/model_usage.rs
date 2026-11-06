use serde::Deserialize;

use crate::parser::lenient;

/// What a `result` event's `modelUsage` reports for one model the turn ran
/// on.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[non_exhaustive]
pub struct ModelUsage {
    /// The model's context window, in tokens, read from `contextWindow`. A
    /// value of any other shape reads as absent.
    #[serde(default, rename = "contextWindow", deserialize_with = "lenient")]
    pub context_window: Option<u64>,
}

impl ModelUsage {
    /// The usage of a model whose context window is `tokens`.
    pub fn with_context_window(mut self, tokens: u64) -> Self {
        self.context_window = Some(tokens);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::ModelUsage;

    #[test]
    fn the_context_window_is_read_from_its_camel_case_name() {
        let usage: ModelUsage = serde_json::from_str(
            r#"{"inputTokens":9,"outputTokens":50,"contextWindow":200000,"maxOutputTokens":32000}"#,
        )
        .expect("a model's usage");

        assert_eq!(usage, ModelUsage::default().with_context_window(200_000));
    }

    #[test]
    fn a_window_of_another_shape_or_none_at_all_is_absent() {
        for json in [
            r#"{}"#,
            r#"{"contextWindow":"wide"}"#,
            r#"{"contextWindow":-1}"#,
        ] {
            let usage: ModelUsage = serde_json::from_str(json).expect("a model's usage");
            assert_eq!(usage.context_window, None, "{json}");
        }
    }
}
