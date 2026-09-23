//! Settings read from `SEARCH_*` environment variables.

use std::env;

fn env_truthy(name: &str, default: bool) -> bool {
    match env::var(name) {
        Ok(value) => matches!(
            value.to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ),
        Err(_) => default,
    }
}

/// Default SearXNG query URL. A SearXNG container that shares a VPN
/// container's network namespace answers on that container's hostname, so the
/// host here is `gluetun` rather than `searxng`.
pub const DEFAULT_SEARXNG_QUERY_URL: &str = "http://gluetun:8080/search?q=<query>&format=json";

/// Web search settings loaded from `SEARCH_*` environment variables.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WebSearchConfig {
    /// Master switch. When false, no request ever reaches SearXNG.
    pub enabled: bool,
    /// Query URL template. `<query>` or `{query}` is replaced with the
    /// URL-encoded search string.
    pub query_url: String,
    /// Max results injected into the prompt (1–20)
    pub result_count: usize,
    /// HTTP timeout for a single SearXNG request
    pub timeout_secs: u64,
}

impl Default for WebSearchConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            query_url: DEFAULT_SEARXNG_QUERY_URL.to_string(),
            result_count: 5,
            timeout_secs: 15,
        }
    }
}

impl WebSearchConfig {
    /// Load from `SEARCH_*` environment variables.
    ///
    /// An unset variable falls back to a deployment default rather than to
    /// [`Default`]: search is on unless `SEARCH_ENABLE_WEB_SEARCH` says
    /// otherwise, and the URL is [`DEFAULT_SEARXNG_QUERY_URL`].
    pub fn from_env() -> Self {
        let result_count = env::var("SEARCH_RESULT_COUNT")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(5)
            .clamp(1, 20);
        let timeout_secs = env::var("SEARCH_TIMEOUT_SECS")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(15)
            .clamp(1, 60);
        Self {
            enabled: env_truthy("SEARCH_ENABLE_WEB_SEARCH", true),
            query_url: env::var("SEARCH_SEARXNG_QUERY_URL")
                .unwrap_or_else(|_| DEFAULT_SEARXNG_QUERY_URL.to_string()),
            result_count,
            timeout_secs,
        }
    }

    /// Whether this message should trigger a SearXNG lookup.
    ///
    /// When the server switch is on, search runs only when the message looks
    /// like it needs current web information. A boolean `metadata.web_search`
    /// value can force a lookup on or off for a single message.
    pub fn requested_for(&self, content: &str, metadata: Option<&serde_json::Value>) -> bool {
        if !self.enabled || self.query_url.trim().is_empty() {
            return false;
        }
        match metadata.and_then(|metadata| metadata.get("web_search")) {
            Some(value) if value.is_boolean() => value.as_bool() == Some(true),
            _ => crate::client::needs_web_search(content),
        }
    }
}
