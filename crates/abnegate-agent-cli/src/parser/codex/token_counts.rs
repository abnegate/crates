use abnegate_llm::Usage;
use serde::Deserialize;

/// Codex reports the cached share of the prompt separately, and unlike
/// Anthropic it reports it as part of `input_tokens` rather than beside it, so
/// the cached count is not added again.
#[derive(Debug, Deserialize)]
#[non_exhaustive]
pub struct TokenCounts {
    #[serde(default)]
    pub input_tokens: u32,
    #[serde(default)]
    pub output_tokens: u32,
}

impl From<TokenCounts> for Usage {
    fn from(counts: TokenCounts) -> Self {
        Self::new(counts.input_tokens, counts.output_tokens)
    }
}
