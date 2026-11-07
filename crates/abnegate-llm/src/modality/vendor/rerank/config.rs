use std::fmt;
use std::time::Duration;

use abnegate_secret::SecretValue;
use reqwest::Url;

const DEFAULT_TIMEOUT: Duration = Duration::from_millis(2500);
const DEFAULT_MAXIMUM_DOCUMENT_CHARACTERS: usize = 1200;
const UNPARSABLE_URL: &str = "<unparsable>";

/// How to reach a rerank endpoint that takes `{model, query, documents}` and
/// answers `{results: [{index, relevance_score}]}`, as Cohere-style rerank
/// APIs, an Infinity server, and a LiteLLM or vLLM `/rerank` route do.
///
/// `url` is the whole endpoint, such as `http://localhost:8080/v1/rerank`;
/// nothing is appended to it. Trailing slashes on its path and any fragment are
/// dropped, and a query string is kept. Printing a config never shows its key,
/// nor the URL's user, password, query or fragment.
#[derive(Clone)]
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

impl fmt::Debug for HttpRerankConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HttpRerankConfig")
            .field("url", &printable_url(&self.url))
            .field("model", &self.model)
            .field("api_key", &self.api_key)
            .field("timeout", &self.timeout)
            .field(
                "maximum_document_characters",
                &self.maximum_document_characters,
            )
            .finish()
    }
}

fn printable_url(url: &str) -> String {
    let Ok(mut parsed) = Url::parse(url) else {
        return UNPARSABLE_URL.to_string();
    };
    if parsed.set_username("").is_err() || parsed.set_password(None).is_err() {
        return UNPARSABLE_URL.to_string();
    }
    parsed.set_query(None);
    parsed.set_fragment(None);
    parsed.to_string()
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
