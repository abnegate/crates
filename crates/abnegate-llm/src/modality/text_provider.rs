use async_trait::async_trait;
use futures::Stream;

use crate::modality::StructuredResponse;
use crate::modality::TextRequest;
use crate::modality::TextResponse;
use crate::provider::ProviderError;

/// A source of text, one prompt at a time.
///
/// This is the modality-axis contract a vendor-native client satisfies: a
/// system prompt and a user prompt in, prose or a schema-shaped value out.
/// [`CompletionProvider`](crate::CompletionProvider) is the conversation-axis
/// contract an OpenAI-compatible endpoint or a coding agent satisfies, with a
/// whole message history and tool definitions.
/// [`CompletionBridge`](crate::modality::CompletionBridge) adapts any
/// `CompletionProvider` into a `TextProvider`.
#[async_trait]
pub trait TextProvider: Send + Sync {
    /// The provider's name, as errors and logs report it.
    fn name(&self) -> &str;
    /// Whether the provider can hold an answer to a JSON schema itself,
    /// rather than only being asked to.
    fn supports_structured_output(&self) -> bool;
    /// How many tokens of context the model behind this provider takes.
    fn maximum_context_tokens(&self) -> u32;

    /// Answer `request` in prose.
    async fn complete(&self, request: &TextRequest) -> Result<TextResponse, ProviderError>;
    /// Ask for a value of the shape `request.response_format` describes.
    async fn complete_structured(
        &self,
        request: &TextRequest,
    ) -> Result<StructuredResponse, ProviderError>;
    /// Answer `request` in prose, a fragment at a time. A provider that
    /// cannot stream fails with [`ProviderError::unsupported`].
    async fn stream_complete(
        &self,
        request: &TextRequest,
    ) -> Result<Box<dyn Stream<Item = Result<String, ProviderError>> + Send + Unpin>, ProviderError>;
}
