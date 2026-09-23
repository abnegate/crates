use async_trait::async_trait;
use futures::Stream;

use crate::modality::{TextRequest, TextResponse};
use crate::provider::ProviderError;

#[async_trait]
pub trait TextProvider: Send + Sync {
    fn name(&self) -> &str;
    fn supports_structured_output(&self) -> bool;
    fn max_context_tokens(&self) -> u32;

    async fn complete(&self, request: &TextRequest) -> Result<TextResponse, ProviderError>;
    async fn complete_structured(
        &self,
        request: &TextRequest,
    ) -> Result<serde_json::Value, ProviderError>;
    async fn stream_complete(
        &self,
        request: &TextRequest,
    ) -> Result<Box<dyn Stream<Item = Result<String, ProviderError>> + Send + Unpin>, ProviderError>;
}
