//! Embeddings from any OpenAI-compatible `/embeddings` endpoint: LiteLLM,
//! vLLM, Ollama's `/v1`, a gateway, or OpenAI itself.

mod config;
mod item;
mod request;
mod response;

use std::sync::Arc;
use std::sync::atomic::AtomicU32;
use std::sync::atomic::Ordering;

use async_trait::async_trait;
use reqwest::Url;

pub use crate::modality::vendor::openai_compatible::config::OpenAiCompatibleEmbeddingConfig;

use crate::client::reporter::conceal_key;
use crate::modality::EmbeddingProvider;
use crate::modality::vendor::endpoint::endpoint;
use crate::modality::vendor::openai_compatible::request::Request;
use crate::modality::vendor::openai_compatible::response::Response;
use crate::modality::vendor::transport::Transport;
use crate::provider::ProviderError;

const NAME: &str = "openai-compatible";
const EMBEDDINGS_PATH: &str = "/embeddings";
const UNLEARNED_DIMENSIONS: u32 = 0;

/// An [`EmbeddingProvider`] over an OpenAI-compatible `/embeddings`
/// endpoint.
///
/// Every batch is one request, and the answer is put back in input order by
/// each item's `index`. Embeddings are passed on at whatever width the model
/// returns: nothing is truncated, padded or compared with
/// [`EmbeddingProvider::dimensions`]. An empty embedding is refused as
/// [`ProviderError::Parse`].
///
/// Without [`OpenAiCompatibleEmbeddingConfig::dimensions`],
/// [`EmbeddingProvider::dimensions`] reads 0 until the first answer, then the
/// width of that answer for as long as the provider lives, so a caller
/// comparing a later embedding's length against it sees the model change.
///
/// A refusal keeps its HTTP status and the endpoint's own wording, with the
/// configured key and any other credential scrubbed out, so a caller can tell
/// a rejected key (401, 403), a throttle (429) and an input past the model's
/// context window apart. The client never follows a redirect.
#[derive(Debug, Clone)]
pub struct OpenAiCompatibleEmbeddingProvider {
    config: OpenAiCompatibleEmbeddingConfig,
    url: Url,
    transport: Transport,
    received_dimensions: Arc<AtomicU32>,
}

impl OpenAiCompatibleEmbeddingProvider {
    /// A provider posting to `<base_url>/embeddings`, refusing a base URL
    /// that does not parse or is not `http` or `https`.
    pub fn new(config: OpenAiCompatibleEmbeddingConfig) -> Result<Self, ProviderError> {
        let url = endpoint(&config.base_url, EMBEDDINGS_PATH)?;
        let transport = Transport::with_timeout(config.timeout);
        Ok(Self {
            config,
            url,
            transport,
            received_dimensions: Arc::default(),
        })
    }

    fn bearer(&self) -> Option<&str> {
        self.config
            .api_key
            .as_ref()
            .map(|key| key.expose().trim())
            .filter(|key| !key.is_empty())
    }

    fn concealed(&self, error: ProviderError) -> ProviderError {
        match (&self.config.api_key, error) {
            (Some(key), ProviderError::Api { status, message }) => {
                ProviderError::api(status, conceal_key(&message, key.expose()))
            }
            (_, error) => error,
        }
    }
}

#[async_trait]
impl EmbeddingProvider for OpenAiCompatibleEmbeddingProvider {
    fn name(&self) -> &str {
        NAME
    }

    fn dimensions(&self) -> u32 {
        self.config
            .dimensions
            .unwrap_or_else(|| self.received_dimensions.load(Ordering::Relaxed))
    }

    fn model(&self) -> &str {
        &self.config.model
    }

    fn maximum_input_tokens(&self) -> u32 {
        self.config.maximum_input_tokens
    }

    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, ProviderError> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        let mut request = self.transport.post(self.url.clone()).json(&Request {
            model: &self.config.model,
            input: texts,
            dimensions: self.config.dimensions,
        });
        if let Some(key) = self.bearer() {
            request = request.bearer_auth(key);
        }

        let body = self
            .transport
            .send(request)
            .await
            .map_err(|error| self.concealed(error))?;
        let response: Response = serde_json::from_value(body).map_err(ProviderError::parse)?;
        let embeddings = response.into_embeddings(texts.len())?;

        if let Some(embedding) = embeddings.first() {
            let width = u32::try_from(embedding.len()).unwrap_or(u32::MAX);
            let _ = self.received_dimensions.compare_exchange(
                UNLEARNED_DIMENSIONS,
                width,
                Ordering::Relaxed,
                Ordering::Relaxed,
            );
        }
        Ok(embeddings)
    }

    async fn embed_single(&self, text: &str) -> Result<Vec<f32>, ProviderError> {
        self.embed(&[text.to_string()])
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| ProviderError::parse("no embedding came back"))
    }
}

#[cfg(test)]
mod tests;
