//! A provider backed by an OpenAI-compatible endpoint.

use std::fmt;

use async_trait::async_trait;

use crate::client::LlmClient;
use crate::client::LlmConfig;
use crate::provider::capabilities::Capabilities;
use crate::provider::completion::{
    Completion, CompletionProvider, CompletionRequest, ProviderKind,
};
use crate::provider::credential::Credential;
use crate::provider::error::ProviderError;

/// Wraps [`LlmClient`] so an HTTP route and a coding agent can sit behind the
/// same handle.
pub struct HttpProvider {
    name: String,
    client: LlmClient,
    capabilities: Capabilities,
}

impl HttpProvider {
    /// Wrap a client the caller has already configured.
    pub fn new(name: impl Into<String>, client: LlmClient) -> Self {
        Self {
            name: name.into(),
            client,
            capabilities: Capabilities {
                cost_reporting: true,
                ..Capabilities::NONE
            },
        }
    }

    /// Build a client for one endpoint, exposing the credential exactly once.
    pub fn connect(
        name: impl Into<String>,
        base_url: impl Into<String>,
        credential: &Credential,
        model: impl Into<String>,
    ) -> Self {
        let config = LlmConfig {
            base_url: base_url.into(),
            api_key: credential.expose().unwrap_or_default().to_string(),
            default_model: model.into(),
            ..LlmConfig::default()
        };
        Self::new(name, LlmClient::new(config))
    }

    /// Declare what the endpoint behind this provider actually supports.
    pub fn with_capabilities(mut self, capabilities: Capabilities) -> Self {
        self.capabilities = capabilities;
        self
    }

    pub fn client(&self) -> &LlmClient {
        &self.client
    }
}

/// The client is deliberately not printed. [`LlmConfig`] redacts its own key,
/// and this impl keeps that true even if the client later grows a field that
/// does not.
impl fmt::Debug for HttpProvider {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HttpProvider")
            .field("name", &self.name)
            .field("base_url", &self.client.config().base_url)
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl CompletionProvider for HttpProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn kind(&self) -> ProviderKind {
        ProviderKind::Http
    }

    fn capabilities(&self) -> Capabilities {
        self.capabilities
    }

    async fn complete(&self, request: CompletionRequest<'_>) -> Result<Completion, ProviderError> {
        let response = self
            .client
            .chat_with_options(
                request.model,
                request.messages,
                request.tools,
                request.options,
            )
            .await
            .map_err(|source| ProviderError::Http {
                provider: self.name.clone(),
                source,
            })?;

        let usage = response.usage;
        let choice =
            response.choices.into_iter().next().ok_or_else(|| {
                ProviderError::agent(&self.name, "the provider returned no choices")
            })?;

        Ok(Completion {
            provider: self.name.clone(),
            message: choice.message,
            usage,
            finish_reason: choice.finish_reason,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::HttpProvider;
    use crate::provider::capabilities::Capabilities;
    use crate::provider::completion::{CompletionProvider, ProviderKind};
    use crate::provider::credential::Credential;

    #[test]
    fn debug_never_prints_the_credential() {
        let provider = HttpProvider::connect(
            "gateway",
            "http://127.0.0.1:4000/v1",
            &Credential::key("GATEWAY_KEY", "sk-notarealkey-abcdefghijklmnop"),
            "qwen",
        );

        let rendered = format!("{provider:?}");
        assert!(
            !rendered.contains("sk-notarealkey"),
            "credential leaked: {rendered}"
        );
        assert!(rendered.contains("gateway"));
        assert!(rendered.contains("127.0.0.1"));
    }

    #[test]
    fn connect_carries_the_endpoint_and_model_through() {
        let provider = HttpProvider::connect(
            "gateway",
            "http://127.0.0.1:4000/v1",
            &Credential::Inherited,
            "qwen3",
        );

        assert_eq!(provider.name(), "gateway");
        assert_eq!(provider.kind(), ProviderKind::Http);
        assert_eq!(provider.client().config().default_model, "qwen3");
        assert!(provider.client().config().api_key.is_empty());
    }

    #[test]
    fn an_endpoint_declares_what_it_supports() {
        let provider = HttpProvider::connect(
            "gateway",
            "http://127.0.0.1:4000/v1",
            &Credential::Inherited,
            "qwen3",
        );

        assert!(provider.capabilities().cost_reporting);
        assert!(!provider.capabilities().tool_permissions);

        let permissive = HttpProvider::connect(
            "gateway",
            "http://127.0.0.1:4000/v1",
            &Credential::Inherited,
            "qwen3",
        )
        .with_capabilities(Capabilities::ALL);

        assert!(permissive.capabilities().satisfies(Capabilities::ALL));
    }
}
