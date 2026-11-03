use super::Source;

/// Why an endpoint with no configured context has no known capacity.
const UNDISCLOSED: &str = "The endpoint publishes no deployment metadata.";

/// Why an endpoint's configured context is used as its capacity.
const ASSUMED: &str =
    "The endpoint publishes no deployment metadata, so the configured context is assumed.";

/// Why a requested context is smaller than what was asked for, when the
/// model's reported native capacity cut it.
pub(super) const BOUNDED_BY_NATIVE: &str =
    "The requested context allocation is bounded by the model's reported native capacity.";

/// Why a requested context is smaller than what was asked for, when the
/// context the model is loaded with cut it.
const BOUNDED_BY_RUNTIME: &str =
    "The requested context allocation is bounded by the model's loaded runtime context.";

/// Why a requested context is smaller than what was asked for, when a
/// configured or otherwise resolved capacity cut it.
const BOUNDED_BY_RESOLVED: &str =
    "The requested context allocation is bounded by the capacity resolved for this deployment.";

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
    /// Whether the engine declared it can read images; `None` when it has not
    /// said.
    pub vision: Option<bool>,
    /// Why the limit is what it is, when it is not simply reported.
    pub reason: Option<String>,
    /// The deployment the alias resolved to.
    pub identity: String,
}

impl Capacity {
    /// `identity`'s context `limit`, read from `source`, with no Ollama
    /// override, no reasoning, undeclared vision and no reason given.
    pub fn new(identity: impl Into<String>, limit: Option<u64>, source: Source) -> Self {
        Self {
            limit,
            source,
            ollama: None,
            reasoning: false,
            vision: None,
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

    /// The same capacity, for an engine that declared it can or cannot read
    /// images.
    pub fn with_vision(mut self, vision: bool) -> Self {
        self.vision = Some(vision);
        self
    }

    /// The same capacity, saying why its limit is what it is.
    pub fn with_reason(mut self, reason: impl Into<String>) -> Self {
        self.reason = Some(reason.into());
        self
    }

    /// The same capacity, narrowed to the context window an operator chose
    /// for one chat.
    ///
    /// The choice never exceeds the resolved limit; when that cuts it down,
    /// the reason names what bounded it, and otherwise the resolved reason
    /// stands. A capacity with no Ollama override gains none, so a
    /// third-party endpoint is never sent `num_ctx`. No choice, or a choice
    /// of zero, leaves the capacity as resolved.
    pub fn with_request(self, requested: Option<u64>) -> Self {
        let Some(requested) = requested.filter(|value| *value > 0) else {
            return self;
        };
        let limit = self
            .limit
            .map_or(requested, |ceiling| requested.min(ceiling));
        let reason = if limit < requested {
            Some(self.bound().into())
        } else {
            self.reason
        };
        Self {
            limit: Some(limit),
            source: Source::Configured,
            ollama: self.ollama.map(|_| limit),
            reason,
            ..self
        }
    }

    fn bound(&self) -> &'static str {
        match self.source {
            Source::Runtime => BOUNDED_BY_RUNTIME,
            Source::Provider => BOUNDED_BY_NATIVE,
            _ => BOUNDED_BY_RESOLVED,
        }
    }

    pub(super) fn unknown(model: &str, reason: &str) -> Self {
        Self::new(model, None, Source::Unknown).with_reason(reason)
    }

    pub(super) fn assumed(model: &str, configured: Option<u64>) -> Self {
        match configured {
            Some(limit) => Self::new(model, Some(limit), Source::Configured).with_reason(ASSUMED),
            None => Self::unknown(model, UNDISCLOSED),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local(limit: u64) -> Capacity {
        Capacity::new("qwen", Some(limit), Source::Provider).with_ollama(limit)
    }

    #[test]
    fn vision_is_unknown_for_an_unknown_capacity() {
        assert_eq!(Capacity::unknown("auto", "unresolved").vision, None);
    }

    #[test]
    fn vision_is_what_the_engine_declared() {
        let capacity = Capacity::new("gemma3", Some(16_384), Source::Provider);
        assert_eq!(capacity.vision, None);
        assert_eq!(capacity.clone().with_vision(true).vision, Some(true));
        assert_eq!(capacity.with_vision(false).vision, Some(false));
    }

    #[test]
    fn an_assumed_capacity_is_the_configured_context_without_a_num_ctx() {
        let capacity = Capacity::assumed("gpt-4o", Some(32_768));
        assert_eq!(capacity.limit, Some(32_768));
        assert_eq!(capacity.source, Source::Configured);
        assert_eq!(capacity.ollama, None);
        assert_eq!(capacity.vision, None);
        assert_eq!(capacity.reason.as_deref(), Some(ASSUMED));

        let unknown = Capacity::assumed("gpt-4o", None);
        assert_eq!(unknown.limit, None);
        assert_eq!(unknown.source, Source::Unknown);
        assert_eq!(unknown.reason.as_deref(), Some(UNDISCLOSED));
    }

    #[test]
    fn a_chosen_window_caps_ollama_below_native() {
        let capacity = local(262_144).with_request(Some(32_768));
        assert_eq!(capacity.limit, Some(32_768));
        assert_eq!(capacity.ollama, Some(32_768));
        assert_eq!(capacity.source, Source::Configured);
        assert!(capacity.reason.is_none());
    }

    #[test]
    fn a_chosen_window_never_exceeds_native() {
        let capacity = local(32_768).with_request(Some(262_144));
        assert_eq!(capacity.limit, Some(32_768));
        assert_eq!(capacity.ollama, Some(32_768));
        assert_eq!(capacity.reason.as_deref(), Some(BOUNDED_BY_NATIVE));
    }

    #[test]
    fn a_chosen_window_cut_by_the_loaded_runtime_says_so() {
        let capacity = Capacity::new("qwen", Some(8_192), Source::Runtime)
            .with_ollama(8_192)
            .with_request(Some(32_768));
        assert_eq!(capacity.limit, Some(8_192));
        assert_eq!(capacity.ollama, Some(8_192));
        assert_eq!(capacity.reason.as_deref(), Some(BOUNDED_BY_RUNTIME));
    }

    #[test]
    fn a_chosen_window_cut_by_a_configured_context_says_so() {
        let capacity = Capacity::assumed("gpt-4o", Some(32_768)).with_request(Some(65_536));
        assert_eq!(capacity.limit, Some(32_768));
        assert_eq!(capacity.ollama, None);
        assert_eq!(capacity.reason.as_deref(), Some(BOUNDED_BY_RESOLVED));
    }

    #[test]
    fn a_chosen_window_that_is_not_cut_keeps_the_resolved_reason() {
        let capacity = Capacity::assumed("gpt-4o", Some(32_768)).with_request(Some(8_192));
        assert_eq!(capacity.limit, Some(8_192));
        assert_eq!(capacity.source, Source::Configured);
        assert_eq!(capacity.reason.as_deref(), Some(ASSUMED));

        let exact = Capacity::assumed("gpt-4o", Some(32_768)).with_request(Some(32_768));
        assert_eq!(exact.reason.as_deref(), Some(ASSUMED));
    }

    #[test]
    fn a_chosen_window_does_not_invent_ollama_options_for_a_third_party() {
        let capacity =
            Capacity::new("gpt", Some(128_000), Source::Provider).with_request(Some(8_192));
        assert_eq!(capacity.limit, Some(8_192));
        assert_eq!(capacity.ollama, None);
    }

    #[test]
    fn a_chosen_window_stands_alone_when_capacity_is_unknown() {
        let capacity = Capacity::unknown("auto", "unresolved").with_request(Some(8_192));
        assert_eq!(capacity.limit, Some(8_192));
        assert_eq!(capacity.source, Source::Configured);
        assert_eq!(capacity.ollama, None);
        assert_eq!(capacity.reason.as_deref(), Some("unresolved"));
    }

    #[test]
    fn a_chosen_window_keeps_what_the_engine_declared() {
        let capacity = local(262_144)
            .with_reasoning(true)
            .with_vision(true)
            .with_request(Some(32_768));
        assert!(capacity.reasoning);
        assert_eq!(capacity.vision, Some(true));
        assert_eq!(capacity.identity, "qwen");
    }

    #[test]
    fn omitting_a_choice_leaves_resolved_capacity() {
        for requested in [None, Some(0)] {
            let capacity = local(262_144).with_request(requested);
            assert_eq!(capacity.limit, Some(262_144), "{requested:?}");
            assert_eq!(capacity.ollama, Some(262_144), "{requested:?}");
            assert_eq!(capacity.source, Source::Provider, "{requested:?}");
        }
    }
}
