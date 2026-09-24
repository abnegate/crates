//! Settings read from `SEARCH_*` environment variables.

use std::env;
use std::fmt;
use std::str::FromStr;
use std::time::Duration;

use abnegate_secret::REDACTED;

/// Default SearXNG query URL: an instance on this machine, on SearXNG's usual
/// port. A deployment that runs SearXNG anywhere else names it in
/// `SEARCH_SEARXNG_QUERY_URL` or [`WebSearchConfig::new`].
pub const DEFAULT_SEARXNG_QUERY_URL: &str = "http://127.0.0.1:8080/search?q=<query>&format=json";

const ENABLED_VARIABLE: &str = "SEARCH_ENABLE_WEB_SEARCH";
const QUERY_URL_VARIABLE: &str = "SEARCH_SEARXNG_QUERY_URL";
const RESULT_COUNT_VARIABLE: &str = "SEARCH_RESULT_COUNT";
const TIMEOUT_VARIABLE: &str = "SEARCH_TIMEOUT_SECONDS";

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
    /// Master switch for search. When false,
    /// [`requested_for`](Self::requested_for) selects no message,
    /// [`SearchContext::new`](crate::SearchContext::new) reports search as
    /// disabled, and [`SearxngClient::search`](crate::SearxngClient::search)
    /// returns [`Error::Disabled`](crate::Error::Disabled) without sending a
    /// request.
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

    /// Read from `SEARCH_*` environment variables. Each one that is unset, or
    /// does not parse, keeps its [`Default`] value, so with none set search is
    /// off.
    ///
    /// - `SEARCH_ENABLE_WEB_SEARCH`: `1`, `true`, `yes` or `on`, in any case,
    ///   switches search on; any other value leaves it off.
    /// - `SEARCH_SEARXNG_QUERY_URL`: the query URL template, by default
    ///   [`DEFAULT_SEARXNG_QUERY_URL`].
    /// - `SEARCH_RESULT_COUNT`: the most hits kept, held to 1–20.
    /// - `SEARCH_TIMEOUT_SECONDS`: the request timeout in whole seconds, held
    ///   to 1–60.
    pub fn from_environment() -> Self {
        Self::load(|name| env::var(name).ok())
    }

    /// [`from_environment`](Self::from_environment), reading each variable
    /// through `read`.
    fn load(read: impl Fn(&str) -> Option<String>) -> Self {
        let default = Self::default();
        Self {
            enabled: read(ENABLED_VARIABLE).map_or(default.enabled, |value| truthy(&value)),
            query_url: read(QUERY_URL_VARIABLE).unwrap_or(default.query_url),
            result_count: parsed(&read, RESULT_COUNT_VARIABLE).unwrap_or(default.result_count),
            timeout: parsed(&read, TIMEOUT_VARIABLE)
                .map(Duration::from_secs)
                .unwrap_or(default.timeout),
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

fn truthy(value: &str) -> bool {
    TRUTHY.contains(&value.to_ascii_lowercase().as_str())
}

fn parsed<T: FromStr>(read: &impl Fn(&str) -> Option<String>, name: &str) -> Option<T> {
    read(name).and_then(|value| value.parse().ok())
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::net::IpAddr;
    use std::process::Command;

    use super::*;
    use crate::context::SearchContext;
    use crate::query::build_search_url;

    const CHILD: &str = "ABNEGATE_SEARCH_TEST_CHILD";

    fn environment(variables: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> + use<> {
        let variables: HashMap<String, String> = variables
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect();
        move |name| variables.get(name).cloned()
    }

    /// Re-run the test `name` in a child test process whose environment is
    /// `PATH` plus `environment`, so a test can shape what
    /// [`WebSearchConfig::from_environment`] reads without touching this
    /// process. Returns whether this call was the parent, which has nothing
    /// left to do once the child passes.
    fn delegated_to_child(name: &str, environment: &[(&str, &str)]) -> bool {
        if env::var(CHILD).as_deref() == Ok(name) {
            return false;
        }
        let output = Command::new(env::current_exe().expect("the test binary"))
            .args(["--exact", name, "--nocapture"])
            .env_clear()
            .env("PATH", env::var_os("PATH").unwrap_or_default())
            .env(CHILD, name)
            .envs(environment.iter().copied())
            .output()
            .expect("the child runs");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success(),
            "{stdout}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            stdout.contains("1 passed"),
            "the child ran no test, so it proved nothing\n{stdout}"
        );
        true
    }

    #[test]
    fn from_environment_is_switched_off_when_nothing_is_set() {
        const NAME: &str = "config::tests::from_environment_is_switched_off_when_nothing_is_set";
        if delegated_to_child(NAME, &[]) {
            return;
        }

        let config = WebSearchConfig::from_environment();
        assert!(!config.enabled, "search switched itself on: {config:?}");
        assert_eq!(config, WebSearchConfig::default());
        assert_eq!(SearchContext::new(&config), SearchContext::Disabled);
        assert!(!config.requested_for("What is the latest news on Rust?", None));
    }

    #[test]
    fn from_environment_reads_the_timeout_in_seconds_under_its_spelled_out_name() {
        const NAME: &str = "config::tests::from_environment_reads_the_timeout_in_seconds_under_its_spelled_out_name";
        if delegated_to_child(NAME, &[("SEARCH_TIMEOUT_SECONDS", "42")]) {
            return;
        }

        assert_eq!(
            WebSearchConfig::from_environment().timeout,
            Duration::from_secs(42)
        );
    }

    #[test]
    fn only_the_documented_variables_are_read() {
        let asked = RefCell::new(Vec::new());
        WebSearchConfig::load(|name| {
            asked.borrow_mut().push(name.to_string());
            None
        });

        let mut asked = asked.into_inner();
        asked.sort();
        assert_eq!(
            asked,
            [
                "SEARCH_ENABLE_WEB_SEARCH",
                "SEARCH_RESULT_COUNT",
                "SEARCH_SEARXNG_QUERY_URL",
                "SEARCH_TIMEOUT_SECONDS",
            ]
        );
    }

    #[test]
    fn every_variable_reaches_its_field() {
        let config = WebSearchConfig::load(environment(&[
            ("SEARCH_ENABLE_WEB_SEARCH", "On"),
            (
                "SEARCH_SEARXNG_QUERY_URL",
                "http://searxng.example.test/search?q=<query>&format=json",
            ),
            ("SEARCH_RESULT_COUNT", "7"),
            ("SEARCH_TIMEOUT_SECONDS", "9"),
        ]));
        assert_eq!(
            config,
            WebSearchConfig::new("http://searxng.example.test/search?q=<query>&format=json")
                .with_result_count(7)
                .with_timeout(Duration::from_secs(9))
        );
    }

    #[test]
    fn only_a_truthy_value_switches_search_on() {
        for value in ["1", "true", "TRUE", "yes", "on"] {
            let config = WebSearchConfig::load(environment(&[("SEARCH_ENABLE_WEB_SEARCH", value)]));
            assert!(config.enabled, "{value} left search off");
        }
        for value in ["0", "false", "off", "", "enabled"] {
            let config = WebSearchConfig::load(environment(&[("SEARCH_ENABLE_WEB_SEARCH", value)]));
            assert!(!config.enabled, "{value} switched search on");
        }
    }

    #[test]
    fn a_value_that_does_not_parse_keeps_the_default() {
        let config = WebSearchConfig::load(environment(&[
            ("SEARCH_RESULT_COUNT", "many"),
            ("SEARCH_TIMEOUT_SECONDS", "1.5"),
        ]));
        assert_eq!(config, WebSearchConfig::default());
    }

    #[test]
    fn the_default_instance_is_on_this_machine() {
        assert_eq!(
            DEFAULT_SEARXNG_QUERY_URL,
            "http://127.0.0.1:8080/search?q=<query>&format=json"
        );
        assert_eq!(
            WebSearchConfig::default().query_url,
            DEFAULT_SEARXNG_QUERY_URL
        );

        let url = reqwest::Url::parse(&build_search_url(DEFAULT_SEARXNG_QUERY_URL, "rust", None))
            .expect("the default is a URL");
        let host = url.host_str().and_then(|host| host.parse::<IpAddr>().ok());
        assert!(
            host.is_some_and(|address| address.is_loopback()),
            "the default sends queries off this machine: {url}"
        );
    }

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

        let loaded = WebSearchConfig::load(environment(&[
            ("SEARCH_RESULT_COUNT", "0"),
            ("SEARCH_TIMEOUT_SECONDS", "86400"),
        ]));
        assert_eq!(
            (loaded.result_count, loaded.timeout),
            (MINIMUM_RESULT_COUNT, MAXIMUM_TIMEOUT)
        );
    }
}
