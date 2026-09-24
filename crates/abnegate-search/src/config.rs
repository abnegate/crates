//! Settings read from `SEARCH_*` environment variables.

use std::env;
use std::fmt;
use std::str::FromStr;
use std::time::Duration;

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
const MINIMUM_RESULT_COUNT: usize = 1;
const MAXIMUM_RESULT_COUNT: usize = 20;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(15);
const MINIMUM_TIMEOUT: Duration = Duration::from_secs(1);
const MAXIMUM_TIMEOUT: Duration = Duration::from_secs(60);

/// Web search settings, built with [`new`](Self::new) or read by
/// [`from_environment`](Self::from_environment).
///
/// [`Default`] is switched off and points at [`DEFAULT_SEARXNG_QUERY_URL`].
/// `Debug` leaves out the query URL, which may carry a credential.
#[derive(Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct WebSearchConfig {
    /// Master switch. When false, no request ever reaches SearXNG.
    pub enabled: bool,
    /// Query URL template. `<query>` or `{query}` is replaced with the
    /// URL-encoded search string.
    pub query_url: String,
    /// Max results injected into the prompt, held to 1–20 by the client.
    pub result_count: usize,
    /// HTTP timeout for a single SearXNG request, held to 1–60 seconds by the
    /// client.
    pub timeout: Duration,
}

impl Default for WebSearchConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            query_url: DEFAULT_SEARXNG_QUERY_URL.to_string(),
            result_count: DEFAULT_RESULT_COUNT,
            timeout: DEFAULT_TIMEOUT,
        }
    }
}

impl WebSearchConfig {
    /// Search switched on, against the instance `query_url` names, with the
    /// default result count and timeout.
    pub fn new(query_url: impl Into<String>) -> Self {
        Self {
            enabled: true,
            query_url: query_url.into(),
            ..Self::default()
        }
    }

    /// This config with the master switch set to `enabled`.
    pub fn with_enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// This config keeping at most `result_count` hits, held to 1–20 by the
    /// client.
    pub fn with_result_count(mut self, result_count: usize) -> Self {
        self.result_count = result_count;
        self
    }

    /// This config giving each request `timeout`, held to 1–60 seconds by the
    /// client.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Load from `SEARCH_*` environment variables.
    ///
    /// An unset variable falls back to a deployment default rather than to
    /// [`Default`]: search is on unless `SEARCH_ENABLE_WEB_SEARCH` says
    /// otherwise, and the URL is [`DEFAULT_SEARXNG_QUERY_URL`].
    pub fn from_environment() -> Self {
        Self {
            enabled: truthy(ENABLED_VARIABLE, true),
            query_url: env::var(QUERY_URL_VARIABLE)
                .unwrap_or_else(|_| DEFAULT_SEARXNG_QUERY_URL.to_string()),
            result_count: parsed(RESULT_COUNT_VARIABLE).unwrap_or(DEFAULT_RESULT_COUNT),
            timeout: parsed(TIMEOUT_VARIABLE)
                .map(Duration::from_secs)
                .unwrap_or(DEFAULT_TIMEOUT),
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
        self.result_count = self
            .result_count
            .clamp(MINIMUM_RESULT_COUNT, MAXIMUM_RESULT_COUNT);
        self.timeout = self.timeout.clamp(MINIMUM_TIMEOUT, MAXIMUM_TIMEOUT);
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
            .field("timeout", &self.timeout)
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
        let config = WebSearchConfig::new(
            "https://searcher:hunter2@search.example.test/search?token=abc&q=<query>",
        );
        let rendered = format!("{config:?}");
        for leaked in ["hunter2", "token=abc", "search.example.test"] {
            assert!(!rendered.contains(leaked), "leaked {leaked}: {rendered}");
        }
        assert!(rendered.contains(REDACTED));
    }

    #[test]
    fn out_of_range_settings_are_held_to_their_bounds() {
        let low = WebSearchConfig::default()
            .with_result_count(0)
            .with_timeout(Duration::ZERO)
            .bounded();
        assert_eq!(
            (low.result_count, low.timeout),
            (MINIMUM_RESULT_COUNT, MINIMUM_TIMEOUT)
        );

        let high = WebSearchConfig::default()
            .with_result_count(500)
            .with_timeout(Duration::from_secs(86_400))
            .bounded();
        assert_eq!(
            (high.result_count, high.timeout),
            (MAXIMUM_RESULT_COUNT, MAXIMUM_TIMEOUT)
        );
    }
}
