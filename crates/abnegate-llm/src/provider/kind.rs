use std::fmt;

/// How a provider reaches the model behind it.
///
/// A caller that must know the difference — a budget that only applies to
/// metered HTTP, a workspace that only a CLI agent can edit — reads this
/// rather than matching on the provider's name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ProviderKind {
    /// An OpenAI-compatible HTTP endpoint.
    Http,
    /// A coding agent CLI driven as a child process.
    Cli,
    /// A router whose arms reach their models in more than one way.
    Mixed,
}

impl ProviderKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Http => "http",
            Self::Cli => "cli",
            Self::Mixed => "mixed",
        }
    }
}

impl fmt::Display for ProviderKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::ProviderKind;

    #[test]
    fn a_provider_kind_renders_its_own_label() {
        assert_eq!(ProviderKind::Http.to_string(), "http");
        assert_eq!(ProviderKind::Cli.to_string(), "cli");
        assert_eq!(ProviderKind::Mixed.to_string(), "mixed");
        assert_ne!(ProviderKind::Http, ProviderKind::Cli);
    }
}
