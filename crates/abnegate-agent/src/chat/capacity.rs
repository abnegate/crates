use super::Source;

/// The effective context limit of one deployment, and where it came from.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Capacity {
    /// Tokens the deployment's context holds, when known.
    pub limit: Option<u64>,
    /// Where `limit` was read from.
    pub source: Source,
    /// Apply only to requests for the alias used to resolve this capacity.
    pub ollama: Option<u64>,
    /// Engine or provider advertised thinking / extended reasoning.
    pub reasoning: bool,
    /// Why the limit is what it is, when it is not simply reported.
    pub reason: Option<String>,
    /// The deployment the alias resolved to.
    pub identity: String,
}

impl Capacity {
    /// `identity`'s context `limit`, read from `source`, with no Ollama
    /// override, no reasoning and no reason given.
    pub fn new(identity: impl Into<String>, limit: Option<u64>, source: Source) -> Self {
        Self {
            limit,
            source,
            ollama: None,
            reasoning: false,
            reason: None,
            identity: identity.into(),
        }
    }

    /// The same capacity, with `tokens` of context requested from Ollama
    /// for the alias it was resolved for.
    pub fn with_ollama(mut self, tokens: u64) -> Self {
        self.ollama = Some(tokens);
        self
    }

    /// The same capacity, for a deployment that does or does not reason.
    pub fn with_reasoning(mut self, reasoning: bool) -> Self {
        self.reasoning = reasoning;
        self
    }

    /// The same capacity, saying why its limit is what it is.
    pub fn with_reason(mut self, reason: impl Into<String>) -> Self {
        self.reason = Some(reason.into());
        self
    }

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
