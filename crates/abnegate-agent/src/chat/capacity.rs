use super::Source;

/// The effective context limit of one deployment, and where it came from.
#[derive(Debug, Clone)]
pub struct Capacity {
    pub limit: Option<u64>,
    pub source: Source,
    /// Apply only to requests for the alias used to resolve this capacity.
    pub ollama: Option<u64>,
    /// Engine or provider advertised thinking / extended reasoning.
    pub reasoning: bool,
    pub reason: Option<String>,
    pub identity: String,
}

impl Capacity {
    pub(super) fn unknown(model: &str, reason: &str) -> Self {
        Self {
            limit: None,
            source: Source::Unknown,
            ollama: None,
            reasoning: false,
            reason: Some(reason.into()),
            identity: model.into(),
        }
    }
}
