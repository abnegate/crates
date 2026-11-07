use std::time::Duration;

use abnegate_secret::SecretValue;

const DEFAULT_TIMEOUT: Duration = Duration::from_millis(2500);
const DEFAULT_MAXIMUM_DOCUMENT_CHARACTERS: usize = 1200;

/// How to reach a rerank endpoint that takes `{model, query, documents}` and
/// answers `{results: [{index, relevance_score}]}`, as Cohere-style rerank
/// APIs, an Infinity server, and a LiteLLM or vLLM `/rerank` route do.
///
/// `url` is the whole endpoint, such as `http://localhost:8080/v1/rerank`;
/// nothing is appended to it. Trailing slashes on its path and any fragment are
/// dropped, and a query string is kept. Printing a config never shows its key,
/// though it does show the URL, so put no credential in the URL.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct HttpRerankConfig {
    /// The endpoint to post every batch to, such as
    /// `http://localhost:8080/v1/rerank`.
    pub url: String,
    /// The model to score with, as the endpoint names it.
    pub model: String,
    /// The bearer token to send, trimmed, or `None` (or a blank key) to send
    /// no `Authorization` header.
    pub api_key: Option<SecretValue>,
    /// The deadline for one request, 2.5 seconds unless set.
    pub timeout: Duration,
    /// The most characters of a document
    /// [`RerankProvider::score`](crate::RerankProvider::score) sends, 1,200
    /// unless set. [`RerankProvider::rerank`](crate::RerankProvider::rerank)
    /// sends every document whole.
    pub maximum_document_characters: usize,
}

impl HttpRerankConfig {
    /// Score with `model` at `url`, with no key, a 2.5 second deadline and
    /// single documents cut to 1,200 characters.
    pub fn new(url: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            model: model.into(),
            api_key: None,
            timeout: DEFAULT_TIMEOUT,
            maximum_document_characters: DEFAULT_MAXIMUM_DOCUMENT_CHARACTERS,
        }
    }

    /// Send `api_key` as a bearer token.
    pub fn with_api_key(mut self, api_key: impl Into<SecretValue>) -> Self {
        self.api_key = Some(api_key.into());
        self
    }

    /// Give up on a request after `timeout`.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Cut a document scored alone to its first `maximum_document_characters`
    /// characters.
    pub fn with_maximum_document_characters(mut self, maximum_document_characters: usize) -> Self {
        self.maximum_document_characters = maximum_document_characters;
        self
    }
}
