//! Settings read from `SEARCH_*` environment variables.

use std::env;
use std::fmt;
use std::str::FromStr;

use abnegate_secret::REDACTED;

/// Default SearXNG query URL. A SearXNG container that shares a VPN
/// container's network namespace answers on that container's hostname, so the
/// host here is `gluetun` rather than `searxng`.
pub const DEFAULT_SEARXNG_QUERY_URL: &str = "http://gluetun:8080/search?q=<query>&format=json";

const ENABLED_VARIABLE: &str = "SEARCH_ENABLE_WEB_SEARCH";
const QUERY_URL_VARIABLE: &str = "SEARCH_SEARXNG_QUERY_URL";
const RESULT_COUNT_VARIABLE: &str = "SEARCH_RESULT_COUNT";
const TIMEOUT_VARIABLE: &str = "SEARCH_TIMEOUT_SECS";

const WEB_SEARCH_METADATA_KEY: &str = "web_search";
const TRUTHY: [&str; 4] = ["1", "true", "yes", "on"];

const DEFAULT_RESULT_COUNT: usize = 5;
const MIN_RESULT_COUNT: usize = 1;
const MAX_RESULT_COUNT: usize = 20;

const DEFAULT_TIMEOUT_SECS: u64 = 15;
const MIN_TIMEOUT_SECS: u64 = 1;
const MAX_TIMEOUT_SECS: u64 = 60;

/// Web search settings loaded from `SEARCH_*` environment variables.
///
/// `Debug` leaves out the query URL, which may carry a credential.
#[derive(Clone, PartialEq, Eq)]
pub struct WebSearchConfig {
    /// Master switch. When false, no request ever reaches SearXNG.
    pub enabled: bool,
    /// Query URL template. `<query>` or `{query}` is replaced with the
    /// URL-encoded search string.
    pub query_url: String,
    /// Max results injected into the prompt, held to 1–20 by the client.
    pub result_count: usize,
    /// HTTP timeout for a single SearXNG request, held to 1–60 by the client.
    pub timeout_secs: u64,
}

impl Default for WebSearchConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            query_url: DEFAULT_SEARXNG_QUERY_URL.to_string(),
            result_count: DEFAULT_RESULT_COUNT,
            timeout_secs: DEFAULT_TIMEOUT_SECS,
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
        Self {
            enabled: truthy(ENABLED_VARIABLE, true),
            query_url: env::var(QUERY_URL_VARIABLE)
                .unwrap_or_else(|_| DEFAULT_SEARXNG_QUERY_URL.to_string()),
            result_count: parsed(RESULT_COUNT_VARIABLE).unwrap_or(DEFAULT_RESULT_COUNT),
            timeout_secs: parsed(TIMEOUT_VARIABLE).unwrap_or(DEFAULT_TIMEOUT_SECS),
        }
        .bounded()
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
        match metadata.and_then(|metadata| metadata.get(WEB_SEARCH_METADATA_KEY)) {
            Some(value) if value.is_boolean() => value.as_bool() == Some(true),
            _ => crate::intent::needs_web_search(content),
        }
    }

    /// This config with the result count and timeout held to their ranges.
    pub(crate) fn bounded(mut self) -> Self {
        self.result_count = self.result_count.clamp(MIN_RESULT_COUNT, MAX_RESULT_COUNT);
        self.timeout_secs = self.timeout_secs.clamp(MIN_TIMEOUT_SECS, MAX_TIMEOUT_SECS);
        self
    }
}

impl fmt::Debug for WebSearchConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WebSearchConfig")
            .field("enabled", &self.enabled)
            .field("query_url", &REDACTED)
            .field("result_count", &self.result_count)
            .field("timeout_secs", &self.timeout_secs)
            .finish()
    }
}

fn truthy(name: &str, default: bool) -> bool {
    match env::var(name) {
        Ok(value) => TRUTHY.contains(&value.to_ascii_lowercase().as_str()),
        Err(_) => default,
    }
}

fn parsed<T: FromStr>(name: &str) -> Option<T> {
    env::var(name).ok().and_then(|value| value.parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_query_url_never_appears_in_debug() {
        let config = WebSearchConfig {
            query_url: "https://searcher:hunter2@search.example.test/search?token=abc&q=<query>"
                .to_string(),
            ..WebSearchConfig::default()
        };
        let rendered = format!("{config:?}");
        for leaked in ["hunter2", "token=abc", "search.example.test"] {
            assert!(!rendered.contains(leaked), "leaked {leaked}: {rendered}");
        }
        assert!(rendered.contains(REDACTED));
    }

    #[test]
    fn out_of_range_settings_are_held_to_their_bounds() {
        let low = WebSearchConfig {
            result_count: 0,
            timeout_secs: 0,
            ..WebSearchConfig::default()
        }
        .bounded();
        assert_eq!(
            (low.result_count, low.timeout_secs),
            (MIN_RESULT_COUNT, MIN_TIMEOUT_SECS)
        );

        let high = WebSearchConfig {
            result_count: 500,
            timeout_secs: 86_400,
            ..WebSearchConfig::default()
        }
        .bounded();
        assert_eq!(
            (high.result_count, high.timeout_secs),
            (MAX_RESULT_COUNT, MAX_TIMEOUT_SECS)
        );
    }
}
