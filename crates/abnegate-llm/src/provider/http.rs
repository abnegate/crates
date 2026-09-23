//! A provider backed by an OpenAI-compatible endpoint.

use std::fmt;

use abnegate_secret::SecretValue;
use async_trait::async_trait;

use crate::client::LlmClient;
use crate::client::LlmConfig;
use crate::provider::capabilities::Capabilities;
use crate::provider::completion::Completion;
use crate::provider::completion_provider::CompletionProvider;
use crate::provider::credential::Credential;
use crate::provider::error::ProviderError;
use crate::provider::kind::ProviderKind;
use crate::provider::request::CompletionRequest;

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
        let api_key = credential
            .secret()
            .cloned()
            .unwrap_or_else(|| SecretValue::new(""));
        Self::new(
            name,
            LlmClient::new(LlmConfig::new(base_url, model, api_key)),
        )
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
            .chat_with_request(request)
            .await
            .map_err(|source| ProviderError::http(&self.name, source))?;

        let usage = response.usage;
        let choice =
            response.choices.into_iter().next().ok_or_else(|| {
                ProviderError::agent(&self.name, "the provider returned no choices")
            })?;

        Ok(Completion::new(self.name.clone(), choice.message)
            .with_usage(usage)
            .with_finish_reason(choice.finish_reason))
    }
}

#[cfg(test)]
mod tests {
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::body_partial_json;
    use wiremock::matchers::method;

    use super::HttpProvider;
    use crate::client::RequestOptions;
    use crate::modality::ResponseFormat;
    use crate::provider::capabilities::Capabilities;
    use crate::provider::completion_provider::CompletionProvider;
    use crate::provider::credential::Credential;
    use crate::provider::kind::ProviderKind;
    use crate::provider::request::CompletionRequest;
    use crate::wire::Message;

    const ECHOED_KEY: &str = "sk-proj-4f9c2a7e1b3d5f6a8c0e2b4d6f8a1c3e";

    #[tokio::test]
    async fn a_response_format_and_temperature_reach_the_endpoint() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(body_partial_json(serde_json::json!({
                "temperature": 0.0,
                "max_tokens": 32,
                "response_format": {
                    "type": "json_schema",
                    "json_schema": { "schema": { "type": "object" } }
                }
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "choices": [{
                    "index": 0,
                    "message": { "role": "assistant", "content": "{\"beats\":3}" },
                    "finish_reason": "stop"
                }],
                "usage": { "prompt_tokens": 5, "completion_tokens": 2 }
            })))
            .expect(1)
            .mount(&server)
            .await;
        let provider =
            HttpProvider::connect("gateway", server.uri(), &Credential::Inherited, "qwen3");
        let messages = [Message::user("hello")];
        let format = ResponseFormat::Json {
            schema: Some(serde_json::json!({ "type": "object" })),
        };

        let completion = provider
            .complete(
                CompletionRequest::new("qwen3", &messages, RequestOptions { reserved: 32 })
                    .with_response_format(&format)
                    .with_temperature(0.0),
            )
            .await
            .unwrap();

        assert_eq!(completion.message.content.as_deref(), Some("{\"beats\":3}"));
        assert_eq!(completion.usage.map(|usage| usage.total_tokens), Some(7));
    }

    #[tokio::test]
    async fn a_rejection_body_echoing_the_key_never_reaches_the_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(401).set_body_string(format!(
                r#"{{"error":{{"message":"Incorrect API key provided: {ECHOED_KEY}"}}}}"#
            )))
            .mount(&server)
            .await;
        let provider = HttpProvider::connect(
            "gateway",
            format!("{}/v1", server.uri()),
            &Credential::key("OPENAI_API_KEY", ECHOED_KEY),
            "gpt-4",
        );
        let messages = [Message::user("hello")];

        let error = provider
            .complete(CompletionRequest::new(
                "gpt-4",
                &messages,
                RequestOptions { reserved: 16 },
            ))
            .await
            .expect_err("a 401 is a failure");

        let rendered = format!("{error} {error:?}");
        assert!(
            !rendered.contains(ECHOED_KEY),
            "the echoed key reached the error: {rendered}"
        );
        assert!(rendered.contains("401"), "the status was lost: {rendered}");
        assert!(
            rendered.contains("[REDACTED]"),
            "nothing was redacted: {rendered}"
        );
    }

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
    fn connect_hands_the_credential_over_without_exposing_it() {
        let provider = HttpProvider::connect(
            "gateway",
            "http://127.0.0.1:4000/v1",
            &Credential::key("GATEWAY_KEY", "sk-notarealkey-abcdefghijklmnop"),
            "qwen3",
        );

        assert_eq!(
            provider.client().config().api_key.expose(),
            "sk-notarealkey-abcdefghijklmnop"
        );
        assert!(!format!("{:?}", provider.client()).contains("sk-notarealkey"));
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
