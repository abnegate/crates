//! The contract every completion provider satisfies.

use std::fmt;

use async_trait::async_trait;

use crate::client::RequestOptions;
use crate::provider::capabilities::Capabilities;
use crate::provider::error::ProviderError;
use crate::wire::Message;
use crate::wire::ToolDefinition;
use crate::wire::Usage;

/// How a provider reaches the model behind it.
///
/// A caller that must know the difference — a budget that only applies to
/// metered HTTP, a workspace that only a CLI agent can edit — reads this
/// rather than matching on the provider's name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    /// An OpenAI-compatible HTTP endpoint.
    Http,
    /// A coding agent CLI driven as a child process.
    Cli,
}

impl ProviderKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Http => "http",
            Self::Cli => "cli",
        }
    }
}

impl fmt::Display for ProviderKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// One completion to run.
///
/// Borrowed for the same reason [`ChatRequest`](crate::ChatRequest) is: the
/// agent loop already owns the conversation and the tool definitions, and a
/// provider that took them by value would clone the whole history once per
/// turn.
#[derive(Debug, Clone, Copy)]
pub struct CompletionRequest<'a> {
    pub model: &'a str,
    pub messages: &'a [Message],
    pub tools: Option<&'a [ToolDefinition]>,
    pub options: RequestOptions,
}

/// One completion's result, normalised across provider kinds.
#[derive(Debug, Clone)]
pub struct Completion {
    /// The provider that actually produced this, which under a fallback chain
    /// is not necessarily the one the caller configured first.
    pub provider: String,
    pub message: Message,
    pub usage: Option<Usage>,
    pub finish_reason: Option<String>,
}

/// A source of chat completions.
///
/// [`Router`](super::Router) implements this over a set of providers, so a
/// consumer holds one handle and never learns whether it is talking to a single
/// model, an A/B split, or a fallback chain.
#[async_trait]
pub trait CompletionProvider: fmt::Debug + Send + Sync {
    /// A stable identifier used in logs, metrics, and [`Completion::provider`].
    fn name(&self) -> &str;

    fn kind(&self) -> ProviderKind;

    /// What this provider supports beyond returning a completion.
    fn capabilities(&self) -> Capabilities {
        Capabilities::NONE
    }

    async fn complete(&self, request: CompletionRequest<'_>) -> Result<Completion, ProviderError>;
}

#[cfg(test)]
mod tests {
    use super::ProviderKind;

    #[test]
    fn a_provider_kind_renders_its_own_label() {
        assert_eq!(ProviderKind::Http.to_string(), "http");
        assert_eq!(ProviderKind::Cli.to_string(), "cli");
        assert_ne!(ProviderKind::Http, ProviderKind::Cli);
    }
}
