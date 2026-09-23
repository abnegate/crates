//! The contract every completion provider satisfies.

use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;

use crate::provider::capabilities::Capabilities;
use crate::provider::completion::Completion;
use crate::provider::error::ProviderError;
use crate::provider::kind::ProviderKind;
use crate::provider::request::CompletionRequest;

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

/// A shared provider is a provider, so an `Arc` handed to a
/// [`Router`](super::Router) can also be handed to anything else that takes
/// one.
#[async_trait]
impl<T: CompletionProvider + ?Sized> CompletionProvider for Arc<T> {
    fn name(&self) -> &str {
        (**self).name()
    }

    fn kind(&self) -> ProviderKind {
        (**self).kind()
    }

    fn capabilities(&self) -> Capabilities {
        (**self).capabilities()
    }

    async fn complete(&self, request: CompletionRequest<'_>) -> Result<Completion, ProviderError> {
        (**self).complete(request).await
    }
}
