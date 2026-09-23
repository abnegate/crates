use async_trait::async_trait;
use futures::Stream;

use crate::modality::{StructuredResponse, TextRequest, TextResponse};
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
    fn name(&self) -> &str;
    fn supports_structured_output(&self) -> bool;
    fn max_context_tokens(&self) -> u32;

    async fn complete(&self, request: &TextRequest) -> Result<TextResponse, ProviderError>;
    /// Ask for a value of the shape `request.response_format` describes.
    async fn complete_structured(
        &self,
        request: &TextRequest,
    ) -> Result<StructuredResponse, ProviderError>;
    async fn stream_complete(
        &self,
        request: &TextRequest,
    ) -> Result<Box<dyn Stream<Item = Result<String, ProviderError>> + Send + Unpin>, ProviderError>;
}
