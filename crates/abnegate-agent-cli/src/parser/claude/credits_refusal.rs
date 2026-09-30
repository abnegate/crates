use crate::parser::claude::CREDITS_REQUIRED;
use crate::parser::claude::LONG_CONTEXT_CREDITS_REQUIRED;

/// claude's own kind for a request the account's usage credits could not
/// fund, the `api_error` of the assistant line it reports one on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CreditsRefusal {
    ModelRequiresUsageCredits,
    LongContextCreditsRequired,
}

impl CreditsRefusal {
    const ALL: [Self; 2] = [
        Self::ModelRequiresUsageCredits,
        Self::LongContextCreditsRequired,
    ];

    fn as_str(self) -> &'static str {
        match self {
            Self::ModelRequiresUsageCredits => "model_requires_usage_credits",
            Self::LongContextCreditsRequired => "long_context_credits_required",
        }
    }

    /// The refusal claude's `kind` names, when it names one.
    pub(crate) fn named(kind: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|known| known.as_str() == kind)
    }

    /// What a failure for this refusal begins with, before claude's words.
    pub(crate) fn marker(self) -> &'static str {
        match self {
            Self::ModelRequiresUsageCredits => CREDITS_REQUIRED,
            Self::LongContextCreditsRequired => LONG_CONTEXT_CREDITS_REQUIRED,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::CreditsRefusal;
    use crate::parser::claude::CREDITS_REQUIRED;
    use crate::parser::claude::LONG_CONTEXT_CREDITS_REQUIRED;

    #[test]
    fn each_refusal_is_named_by_claudes_own_kind() {
        assert_eq!(
            CreditsRefusal::named("model_requires_usage_credits"),
            Some(CreditsRefusal::ModelRequiresUsageCredits)
        );
        assert_eq!(
            CreditsRefusal::named("long_context_credits_required"),
            Some(CreditsRefusal::LongContextCreditsRequired)
        );
        for kind in ["credits_required", "max_output_tokens", "rate_limit", ""] {
            assert_eq!(CreditsRefusal::named(kind), None, "{kind}");
        }
    }

    #[test]
    fn each_refusal_has_a_marker_of_its_own() {
        assert_eq!(
            CreditsRefusal::ModelRequiresUsageCredits.marker(),
            CREDITS_REQUIRED
        );
        assert_eq!(
            CreditsRefusal::LongContextCreditsRequired.marker(),
            LONG_CONTEXT_CREDITS_REQUIRED
        );
        assert!(!LONG_CONTEXT_CREDITS_REQUIRED.contains(CREDITS_REQUIRED));
        assert!(!CREDITS_REQUIRED.contains(LONG_CONTEXT_CREDITS_REQUIRED));
    }
}
