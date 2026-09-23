use abnegate_llm::Usage;
use serde::Deserialize;
use serde::Serialize;

/// Token counts as `claude --output-format stream-json` reports them.
///
/// Anthropic reports cache reads and cache writes separately from fresh input.
/// All three are prompt tokens, and dropping the cached ones understates a
/// long conversation's real prompt size by most of it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CliUsage {
    #[serde(default)]
    pub input_tokens: Option<u64>,
    #[serde(default)]
    pub output_tokens: Option<u64>,
    #[serde(default)]
    pub cache_read_input_tokens: Option<u64>,
    #[serde(default)]
    pub cache_creation_input_tokens: Option<u64>,
}

impl CliUsage {
    /// Every token the model read, fresh or from the cache.
    pub fn prompt_tokens(&self) -> u64 {
        [
            self.input_tokens,
            self.cache_read_input_tokens,
            self.cache_creation_input_tokens,
        ]
        .into_iter()
        .flatten()
        .fold(0, u64::saturating_add)
    }
}

impl From<&CliUsage> for Usage {
    fn from(counts: &CliUsage) -> Self {
        let prompt_tokens = narrow(counts.prompt_tokens());
        let completion_tokens = narrow(counts.output_tokens.unwrap_or_default());
        Self {
            prompt_tokens,
            completion_tokens,
            total_tokens: prompt_tokens.saturating_add(completion_tokens),
        }
    }
}

fn narrow(count: u64) -> u32 {
    u32::try_from(count).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use abnegate_llm::Usage;

    use super::CliUsage;

    #[test]
    fn an_empty_object_reports_nothing() {
        let usage: CliUsage = serde_json::from_str("{}").unwrap();
        assert_eq!(usage, CliUsage::default());
    }

    #[test]
    fn a_partial_report_leaves_the_rest_absent() {
        let usage: CliUsage =
            serde_json::from_str(r#"{"input_tokens": 100, "output_tokens": 50}"#).unwrap();
        assert_eq!(usage.input_tokens, Some(100));
        assert_eq!(usage.output_tokens, Some(50));
        assert!(usage.cache_read_input_tokens.is_none());
        assert!(usage.cache_creation_input_tokens.is_none());
    }

    #[test]
    fn every_counter_is_read() {
        let usage: CliUsage = serde_json::from_str(
            r#"{"input_tokens":10,"output_tokens":20,"cache_read_input_tokens":30,"cache_creation_input_tokens":40}"#,
        )
        .unwrap();
        assert_eq!(usage.input_tokens, Some(10));
        assert_eq!(usage.output_tokens, Some(20));
        assert_eq!(usage.cache_read_input_tokens, Some(30));
        assert_eq!(usage.cache_creation_input_tokens, Some(40));
    }

    #[test]
    fn unknown_fields_are_ignored() {
        let usage: CliUsage = serde_json::from_str(
            r#"{"input_tokens":5,"unknown_field":"ignored","output_tokens":10}"#,
        )
        .unwrap();
        assert_eq!(usage.input_tokens, Some(5));
        assert_eq!(usage.output_tokens, Some(10));
    }

    #[test]
    fn debug_names_the_counters() {
        let usage = CliUsage {
            input_tokens: Some(100),
            output_tokens: Some(200),
            ..CliUsage::default()
        };
        let debug = format!("{usage:?}");
        assert!(debug.contains("input_tokens"));
        assert!(debug.contains("100"));
    }

    #[test]
    fn cached_prompt_tokens_count_as_prompt_tokens() {
        let usage = CliUsage {
            input_tokens: Some(9),
            output_tokens: Some(77),
            cache_read_input_tokens: Some(27_700),
            cache_creation_input_tokens: Some(1_200),
        };

        let normalised = Usage::from(&usage);
        assert_eq!(normalised.prompt_tokens, 9 + 1_200 + 27_700);
        assert_eq!(normalised.completion_tokens, 77);
        assert_eq!(normalised.total_tokens, 9 + 1_200 + 27_700 + 77);
    }

    #[test]
    fn counts_past_the_normalised_width_saturate() {
        let usage = CliUsage {
            input_tokens: Some(u64::MAX),
            output_tokens: Some(u64::from(u32::MAX) + 1),
            ..CliUsage::default()
        };

        let normalised = Usage::from(&usage);
        assert_eq!(normalised.prompt_tokens, u32::MAX);
        assert_eq!(normalised.completion_tokens, u32::MAX);
        assert_eq!(normalised.total_tokens, u32::MAX);
    }
}
