use serde::Deserialize;
use serde::Deserializer;
use serde::Serialize;
use serde::de::Error as _;

/// Token usage statistics.
///
/// A counter a provider leaves out reads as zero, and a missing total as the
/// sum of the other two, because a provider that omits one still sent a
/// usable reply and a missing counter must not discard it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct Usage {
    /// Tokens the prompt took.
    pub prompt_tokens: u32,
    /// Tokens the answer took.
    pub completion_tokens: u32,
    /// Tokens billed in all, which may exceed the other two together.
    pub total_tokens: u32,
}

impl<'de> Deserialize<'de> for Usage {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        if !value.is_object() {
            return Err(D::Error::custom("usage must be an object"));
        }
        let count = |field: &str| {
            value
                .get(field)
                .and_then(serde_json::Value::as_u64)
                .map(|count| u32::try_from(count).unwrap_or(u32::MAX))
        };
        let usage = Self::new(
            count("prompt_tokens").unwrap_or_default(),
            count("completion_tokens").unwrap_or_default(),
        );
        Ok(match count("total_tokens") {
            Some(total) => usage.with_total(total),
            None => usage,
        })
    }
}

impl Usage {
    /// Usage whose total is the prompt and the completion together.
    pub fn new(prompt_tokens: u32, completion_tokens: u32) -> Self {
        Self {
            prompt_tokens,
            completion_tokens,
            total_tokens: prompt_tokens.saturating_add(completion_tokens),
        }
    }

    /// The same counters with a provider-reported total, which can exceed the
    /// sum when the provider also bills reasoning or cached tokens.
    pub fn with_total(mut self, total_tokens: u32) -> Self {
        self.total_tokens = total_tokens;
        self
    }

    /// This usage and `other` together, counter by counter, each stopping at
    /// `u32::MAX` rather than wrapping.
    pub fn plus(self, other: Self) -> Self {
        Self {
            prompt_tokens: self.prompt_tokens.saturating_add(other.prompt_tokens),
            completion_tokens: self
                .completion_tokens
                .saturating_add(other.completion_tokens),
            total_tokens: self.total_tokens.saturating_add(other.total_tokens),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Usage;

    #[test]
    fn usage_reads_every_counter() {
        let json = r#"{
            "prompt_tokens": 100,
            "completion_tokens": 50,
            "total_tokens": 150
        }"#;

        let usage: Usage = serde_json::from_str(json).unwrap();
        assert_eq!(usage.prompt_tokens, 100);
        assert_eq!(usage.completion_tokens, 50);
        assert_eq!(usage.total_tokens, 150);
    }

    #[test]
    fn a_missing_counter_reads_as_zero_rather_than_failing_the_reply() {
        let usage: Usage = serde_json::from_str(r#"{"prompt_tokens": 12}"#).unwrap();
        assert_eq!(usage, Usage::new(12, 0));
        assert_eq!(usage.total_tokens, 12);
    }

    #[test]
    fn a_reported_total_is_kept_even_when_it_exceeds_the_sum() {
        let usage: Usage = serde_json::from_str(
            r#"{"prompt_tokens": 1, "completion_tokens": 2, "total_tokens": 9}"#,
        )
        .unwrap();
        assert_eq!(usage.total_tokens, 9);
    }

    #[test]
    fn a_usage_that_is_not_an_object_is_refused() {
        assert!(serde_json::from_str::<Usage>("\"lots\"").is_err());
        assert_eq!(serde_json::from_str::<Option<Usage>>("null").unwrap(), None);
    }

    #[test]
    fn usage_from_two_turns_adds_up_field_by_field() {
        let first = Usage::new(100, 50);
        let second = Usage::new(7, 3);

        assert_eq!(first.plus(second), Usage::new(107, 53));
        assert_eq!(first.plus(second).total_tokens, 160);
    }

    #[test]
    fn usage_that_would_overflow_stops_at_the_largest_count() {
        let full = Usage::new(u32::MAX, u32::MAX - 1).with_total(u32::MAX);
        let more = Usage::new(1, 5).with_total(2);

        assert_eq!(
            full.plus(more),
            Usage::new(u32::MAX, u32::MAX).with_total(u32::MAX)
        );
    }

    #[test]
    fn a_new_usage_totals_its_counters_without_overflowing() {
        assert_eq!(Usage::new(3, 4).total_tokens, 7);
        assert_eq!(Usage::new(u32::MAX, 1).total_tokens, u32::MAX);
    }
}
