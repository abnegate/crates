use async_trait::async_trait;
use futures::Stream;

use crate::client::RequestOptions;
use crate::modality::{StructuredResponse, TextProvider, TextRequest, TextResponse};
use crate::provider::{CompletionProvider, CompletionRequest, ProviderError};
use crate::wire::Message;

const DEFAULT_MAX_CONTEXT_TOKENS: u32 = 128_000;
const DEFAULT_FINISH_REASON: &str = "stop";

/// Any [`CompletionProvider`] as a [`TextProvider`].
///
/// A text request becomes a system message, when it has a system prompt, and
/// a user message, sent to `model` with the request's temperature, output
/// reservation and response format. Structured output is supported exactly
/// when the provider's [`Capabilities`](crate::Capabilities) say so; either
/// way the answer is parsed as JSON, so an endpoint that ignored the format
/// fails loudly rather than returning prose. A completion provider has no
/// streaming contract, so neither does the bridge.
///
/// ```
/// use abnegate_llm::modality::{CompletionBridge, TextProvider};
/// use abnegate_llm::{Credential, HttpProvider};
///
/// let gateway = HttpProvider::connect(
///     "gateway",
///     "http://127.0.0.1:4000/v1",
///     &Credential::Inherited,
///     "qwen3",
/// );
/// let text = CompletionBridge::new(gateway, "qwen3").with_max_context_tokens(32_768);
/// assert_eq!(text.name(), "gateway");
/// assert_eq!(text.max_context_tokens(), 32_768);
/// ```
#[derive(Debug, Clone)]
pub struct CompletionBridge<P> {
    provider: P,
    model: String,
    max_context_tokens: u32,
}

impl<P: CompletionProvider> CompletionBridge<P> {
    pub fn new(provider: P, model: impl Into<String>) -> Self {
        Self {
            provider,
            model: model.into(),
            max_context_tokens: DEFAULT_MAX_CONTEXT_TOKENS,
        }
    }

    /// How much context the model behind the provider takes. 128,000 unless
    /// set, because the provider has no way to say.
    pub fn with_max_context_tokens(mut self, max_context_tokens: u32) -> Self {
        self.max_context_tokens = max_context_tokens;
        self
    }

    pub fn provider(&self) -> &P {
        &self.provider
    }
}

#[async_trait]
impl<P: CompletionProvider> TextProvider for CompletionBridge<P> {
    fn name(&self) -> &str {
        self.provider.name()
    }

    fn supports_structured_output(&self) -> bool {
        self.provider.capabilities().structured_output
    }

    fn max_context_tokens(&self) -> u32 {
        self.max_context_tokens
    }

    async fn complete(&self, request: &TextRequest) -> Result<TextResponse, ProviderError> {
        let mut messages = Vec::with_capacity(2);
        if !request.system_prompt.is_empty() {
            messages.push(Message::system(request.system_prompt.clone()));
        }
        messages.push(Message::user(request.user_prompt.clone()));

        let mut completion_request = CompletionRequest::new(
            &self.model,
            &messages,
            RequestOptions {
                reserved: request.max_tokens,
            },
        )
        .with_temperature(request.temperature as f32);
        if let Some(format) = &request.response_format {
            completion_request = completion_request.with_response_format(format);
        }

        let completion = self.provider.complete(completion_request).await?;
        let usage = completion.usage.unwrap_or_default();
        Ok(TextResponse {
            content: completion.message.content.unwrap_or_default(),
            model: self.model.clone(),
            input_tokens: usage.prompt_tokens,
            output_tokens: usage.completion_tokens,
            finish_reason: completion
                .finish_reason
                .unwrap_or_else(|| DEFAULT_FINISH_REASON.to_string()),
        })
    }

    async fn complete_structured(
        &self,
        request: &TextRequest,
    ) -> Result<StructuredResponse, ProviderError> {
        StructuredResponse::from_text(self.complete(request).await?)
    }

    async fn stream_complete(
        &self,
        _request: &TextRequest,
    ) -> Result<Box<dyn Stream<Item = Result<String, ProviderError>> + Send + Unpin>, ProviderError>
    {
        Err(ProviderError::unsupported(
            "a completion provider has no streaming contract",
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::modality::{AiClient, ResponseFormat};
    use crate::provider::testing::StubProvider;
    use crate::provider::{Capabilities, CompletionProvider};
    use crate::wire::Role;
    use crate::wire::Usage;

    fn structured() -> Capabilities {
        Capabilities {
            structured_output: true,
            ..Capabilities::NONE
        }
    }

    #[tokio::test]
    async fn a_text_request_reaches_the_provider_as_a_conversation() {
        let stub =
            Arc::new(StubProvider::answering("gateway", "hello").with_usage(Usage::new(9, 2)));
        let bridge = CompletionBridge::new(stub.clone(), "qwen3");
        let mut request = TextRequest::new("be brief", "say hello");
        request.temperature = 0.25;
        request.max_tokens = 64;

        let response = bridge.complete(&request).await.unwrap();

        assert_eq!(response.content, "hello");
        assert_eq!(response.model, "qwen3");
        assert_eq!((response.input_tokens, response.output_tokens), (9, 2));
        let seen = stub.seen().unwrap();
        assert_eq!(seen.model, "qwen3");
        assert_eq!(seen.messages.len(), 2);
        assert_eq!(seen.messages[0].role, Role::System);
        assert_eq!(seen.messages[1].content.as_deref(), Some("say hello"));
        assert_eq!(seen.options.reserved, 64);
        assert_eq!(seen.temperature, Some(0.25));
        assert!(seen.response_format.is_none());
    }

    #[tokio::test]
    async fn a_structured_request_carries_its_schema_and_is_parsed() {
        let stub = Arc::new(
            StubProvider::answering("gateway", r#"{"title":"Requiem","genre":"RPG"}"#)
                .with_capabilities(structured()),
        );
        let bridge = CompletionBridge::new(stub.clone(), "qwen3");
        let mut request = TextRequest::new("", "facts");
        request.response_format = Some(ResponseFormat::Json {
            schema: Some(serde_json::json!({ "type": "object" })),
            strict: false,
        });

        let structured = bridge.complete_structured(&request).await.unwrap();

        assert!(bridge.supports_structured_output());
        assert_eq!(structured.value["title"], "Requiem");
        let seen = stub.seen().unwrap();
        assert_eq!(seen.messages.len(), 1, "an empty system prompt is not sent");
        assert!(matches!(
            seen.response_format,
            Some(ResponseFormat::Json {
                schema: Some(_),
                ..
            })
        ));
    }

    #[tokio::test]
    async fn an_ai_client_can_run_on_any_completion_provider() {
        #[derive(Debug, serde::Deserialize)]
        struct Facts {
            title: String,
        }
        let provider: Arc<dyn CompletionProvider> =
            StubProvider::answering("gateway", r#"{"title":"Requiem"}"#)
                .with_capabilities(structured())
                .shared();
        let client = AiClient::new(Box::new(CompletionBridge::new(provider, "qwen3")));

        let facts: Facts = client
            .complete_structured(
                "the facts",
                &serde_json::json!({ "type": "object" }),
                "sys",
                "usr",
                100,
            )
            .await
            .unwrap();

        assert_eq!(facts.title, "Requiem");
        assert_eq!(client.provider_name(), "gateway");
    }

    #[tokio::test]
    async fn prose_where_json_was_asked_for_is_a_parse_error() {
        let bridge = CompletionBridge::new(StubProvider::answering("gateway", "sure!"), "qwen3");
        let mut request = TextRequest::new("", "facts");
        request.response_format = Some(ResponseFormat::Json {
            schema: None,
            strict: false,
        });

        let error = bridge.complete_structured(&request).await.unwrap_err();

        assert!(!bridge.supports_structured_output());
        assert!(matches!(error, ProviderError::Parse { .. }), "{error:?}");
    }
}
