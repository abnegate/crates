//! Cross-encoder scores from any HTTP rerank endpoint that takes
//! `{model, query, documents}`: a Cohere-style API, an Infinity server, or a
//! LiteLLM or vLLM `/rerank` route.

mod config;
mod hit;
mod request;
mod response;

use std::fmt;

use async_trait::async_trait;
use reqwest::Url;

pub use crate::modality::vendor::rerank::config::HttpRerankConfig;

use crate::client::reporter::conceal_key;
use crate::modality::RerankProvider;
use crate::modality::vendor::endpoint::endpoint;
use crate::modality::vendor::rerank::request::Request;
use crate::modality::vendor::rerank::response::Response;
use crate::modality::vendor::transport::Transport;
use crate::provider::ProviderError;

const NAME: &str = "http-rerank";
const EMPTY_PAIR: &str = "empty rerank pair";

/// A [`RerankProvider`] over an HTTP rerank endpoint.
///
/// Every batch is one `POST` of `{model, query, documents}` to
/// [`HttpRerankConfig::url`], with the key as a bearer token when one is set.
/// The answer is read leniently: each hit's score lands at its `index`,
/// `relevance_score` is read before `score`, a hit without an index or past
/// the documents is skipped, and a document no hit names scores 0. That 0 is
/// not on the model's scale, so on a model that scores below 0, a document no
/// hit named can outrank one it scored. An answer with no hits, or with no hit
/// scoring one of the documents, is refused as [`ProviderError::Parse`].
///
/// A provider does not prove that its endpoint ranks usefully: to rely on the
/// scores, first check that it orders a pair whose order is known.
///
/// A refusal keeps its HTTP status and the endpoint's own wording, and any
/// error from the endpoint has the configured key and any other credential
/// scrubbed out, so a caller can tell a rejected key (401, 403) from a throttle
/// (429). The client never follows a redirect, so a 3xx is a refusal with its
/// status. A request that has no answer within [`HttpRerankConfig::timeout`],
/// connecting included, is a [`ProviderError::Network`]; connecting also gives
/// up after 10 seconds when the timeout is longer.
#[derive(Clone)]
pub struct HttpRerankProvider {
    config: HttpRerankConfig,
    url: Url,
    transport: Transport,
}

impl fmt::Debug for HttpRerankProvider {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HttpRerankProvider")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl HttpRerankProvider {
    /// A provider posting to [`HttpRerankConfig::url`], refusing a URL that
    /// does not parse or is not `http` or `https`.
    pub fn new(config: HttpRerankConfig) -> Result<Self, ProviderError> {
        let url = endpoint(&config.url, "")?;
        let transport = Transport::with_timeout(config.timeout);
        Ok(Self {
            config,
            url,
            transport,
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
            (Some(key), ProviderError::Parse { detail }) => {
                ProviderError::parse(conceal_key(&detail, key.expose()))
            }
            (_, error) => error,
        }
    }
}

#[async_trait]
impl RerankProvider for HttpRerankProvider {
    fn name(&self) -> &str {
        NAME
    }

    fn model(&self) -> &str {
        &self.config.model
    }

    async fn rerank(&self, query: &str, documents: &[String]) -> Result<Vec<f32>, ProviderError> {
        if documents.is_empty() {
            return Ok(Vec::new());
        }

        let mut request = self.transport.post(self.url.clone()).json(&Request {
            model: &self.config.model,
            query,
            documents,
        });
        if let Some(key) = self.bearer() {
            request = request.bearer_auth(key);
        }

        let body = self
            .transport
            .send(request)
            .await
            .map_err(|error| self.concealed(error))?;
        let response: Response = serde_json::from_value(body)
            .map_err(|error| self.concealed(ProviderError::parse(error)))?;
        response.into_scores(documents.len())
    }

    async fn score(&self, query: &str, document: &str) -> Result<f32, ProviderError> {
        let clipped: String = document
            .chars()
            .take(self.config.maximum_document_characters)
            .collect();
        self.rerank(query, &[clipped])
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| ProviderError::parse(EMPTY_PAIR))
    }
}

#[cfg(test)]
mod tests;
