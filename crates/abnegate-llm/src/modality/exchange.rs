const TOKENS_PER_MILLION: f64 = 1_000_000.0;

/// One exchange with a model, as it happened.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Exchange {
    pub provider: String,
    pub model: String,
    /// The schema asked for, when one was.
    pub schema: Option<String>,
    pub system_prompt: String,
    pub user_prompt: String,
    pub answer: String,
    /// Every token sent across the attempts this exchange took.
    pub input_tokens: u32,
    /// Every token received across the attempts this exchange took.
    pub output_tokens: u32,
    pub attempts: u32,
}

impl Exchange {
    /// What this exchange cost, at the given rate per million tokens.
    ///
    /// A single rate cannot express the input/output split every model has, so
    /// this is indicative. The token counts beside it are exact when the
    /// provider reports them.
    pub fn cost_usd(&self, per_million_tokens: f64) -> f64 {
        let tokens = u64::from(self.input_tokens) + u64::from(self.output_tokens);
        tokens as f64 / TOKENS_PER_MILLION * per_million_tokens
    }
}

#[cfg(test)]
mod tests {
    use super::Exchange;

    fn exchange(input_tokens: u32, output_tokens: u32) -> Exchange {
        Exchange {
            provider: "anthropic".into(),
            model: "claude-opus-5".into(),
            schema: None,
            system_prompt: String::new(),
            user_prompt: String::new(),
            answer: String::new(),
            input_tokens,
            output_tokens,
            attempts: 1,
        }
    }

    #[test]
    fn a_cost_is_derived_from_the_tokens_that_were_used() {
        assert!((exchange(500_000, 500_000).cost_usd(25.0) - 25.0).abs() < 1e-9);
    }

    #[test]
    fn a_cost_never_overflows_the_counters() {
        let cost = exchange(u32::MAX, u32::MAX).cost_usd(1.0);
        assert!((cost - 2.0 * f64::from(u32::MAX) / 1_000_000.0).abs() < 1e-6);
    }
}
