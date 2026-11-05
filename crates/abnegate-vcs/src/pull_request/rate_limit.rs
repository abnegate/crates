use reqwest::header::HeaderMap;
use std::time::Duration;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

/// The header GitHub reports the requests allowed in the current window in.
const LIMIT: &str = "x-ratelimit-limit";

/// The header GitHub reports the requests left in the current window in.
pub(super) const REMAINING: &str = "x-ratelimit-remaining";

/// The header GitHub reports the requests made in the current window in.
const USED: &str = "x-ratelimit-used";

/// The header GitHub reports when the current window ends in, as seconds
/// since the Unix epoch.
const RESET: &str = "x-ratelimit-reset";

/// The header GitHub names the limit a request counts against in.
const RESOURCE: &str = "x-ratelimit-resource";

/// What GitHub said of the rate limit a request counts against, each part
/// only where its answer said it in a form this crate reads.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct RateLimit {
    /// The requests the window allows.
    pub limit: Option<u64>,
    /// The requests left in the window.
    pub remaining: Option<u64>,
    /// The requests made in the window.
    pub used: Option<u64>,
    /// When the window ends and the limit is restored.
    pub reset: Option<SystemTime>,
    /// The limit the request counts against, such as `core`, `search` or
    /// `graphql`.
    pub resource: Option<String>,
}

impl RateLimit {
    /// The requests the window allows.
    #[must_use]
    pub fn with_limit(mut self, limit: u64) -> Self {
        self.limit = Some(limit);
        self
    }

    /// The requests left in the window.
    #[must_use]
    pub fn with_remaining(mut self, remaining: u64) -> Self {
        self.remaining = Some(remaining);
        self
    }

    /// The requests made in the window.
    #[must_use]
    pub fn with_used(mut self, used: u64) -> Self {
        self.used = Some(used);
        self
    }

    /// When the window ends and the limit is restored.
    #[must_use]
    pub fn with_reset(mut self, reset: SystemTime) -> Self {
        self.reset = Some(reset);
        self
    }

    /// The limit the request counts against.
    #[must_use]
    pub fn with_resource(mut self, resource: impl Into<String>) -> Self {
        self.resource = Some(resource.into());
        self
    }

    /// What `headers` say of the rate limit. A header that is missing, is not
    /// text, or does not hold what it should once trimmed says nothing, and
    /// a reset past what a [`SystemTime`] holds says nothing either.
    pub(super) fn from_headers(headers: &HeaderMap) -> Self {
        Self {
            limit: count(headers, LIMIT),
            remaining: count(headers, REMAINING),
            used: count(headers, USED),
            reset: count(headers, RESET)
                .and_then(|seconds| UNIX_EPOCH.checked_add(Duration::from_secs(seconds))),
            resource: text(headers, RESOURCE).map(str::to_string),
        }
    }
}

/// The whole number `name` holds in `headers`.
fn count(headers: &HeaderMap, name: &str) -> Option<u64> {
    text(headers, name)?.parse().ok()
}

/// The text `name` holds in `headers`, trimmed, unless nothing is left.
fn text<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    let value = headers.get(name)?.to_str().ok()?.trim();
    (!value.is_empty()).then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::HeaderName;
    use reqwest::header::HeaderValue;

    /// When the window in these tests ends, as seconds since the Unix epoch.
    const RESET_SECONDS: u64 = 1_700_000_000;

    fn headers(pairs: &[(&'static str, HeaderValue)]) -> HeaderMap {
        pairs
            .iter()
            .map(|(name, value)| (HeaderName::from_static(name), value.clone()))
            .collect()
    }

    fn value(text: &'static str) -> HeaderValue {
        HeaderValue::from_static(text)
    }

    #[test]
    fn every_rate_limit_header_is_read() {
        let read = RateLimit::from_headers(&headers(&[
            (LIMIT, value("5000")),
            (REMAINING, value("0")),
            (USED, value("5000")),
            (RESET, value("1700000000")),
            (RESOURCE, value("core")),
        ]));

        assert_eq!(
            read,
            RateLimit {
                limit: Some(5000),
                remaining: Some(0),
                used: Some(5000),
                reset: Some(UNIX_EPOCH + Duration::from_secs(RESET_SECONDS)),
                resource: Some("core".to_string()),
            }
        );

        let partway = RateLimit::from_headers(&headers(&[
            (LIMIT, value("30")),
            (REMAINING, value("21")),
            (USED, value("9")),
            (RESET, value("1700000060")),
            (RESOURCE, value("search")),
        ]));

        assert_eq!(
            partway,
            RateLimit {
                limit: Some(30),
                remaining: Some(21),
                used: Some(9),
                reset: Some(UNIX_EPOCH + Duration::from_secs(RESET_SECONDS + 60)),
                resource: Some("search".to_string()),
            }
        );
    }

    #[test]
    fn a_response_without_rate_limit_headers_knows_nothing() {
        assert_eq!(
            RateLimit::from_headers(&HeaderMap::new()),
            RateLimit::default()
        );
    }

    #[test]
    fn values_are_read_trimmed() {
        let read = RateLimit::from_headers(&headers(&[
            (REMAINING, value(" 42 ")),
            (RESOURCE, value(" search ")),
        ]));

        assert_eq!(read.remaining, Some(42));
        assert_eq!(read.resource.as_deref(), Some("search"));
    }

    #[test]
    fn a_malformed_value_is_unknown() {
        let unreadable = HeaderValue::from_bytes(b"\xff").unwrap();
        for malformed in [
            value("abc"),
            value("-1"),
            value("1.5"),
            value(""),
            value("  "),
            unreadable.clone(),
        ] {
            let read = RateLimit::from_headers(&headers(&[
                (LIMIT, malformed.clone()),
                (REMAINING, malformed.clone()),
                (USED, malformed.clone()),
                (RESET, malformed.clone()),
            ]));

            assert_eq!(read, RateLimit::default(), "{malformed:?}");
        }

        let overflowing =
            RateLimit::from_headers(&headers(&[(RESET, value("18446744073709551615"))]));
        assert_eq!(overflowing.reset, None);

        for resource in [value(""), value("  "), unreadable] {
            let read = RateLimit::from_headers(&headers(&[(RESOURCE, resource.clone())]));

            assert_eq!(read.resource, None, "{resource:?}");
        }
    }

    #[test]
    fn the_builders_set_each_field() {
        let reset = UNIX_EPOCH + Duration::from_secs(RESET_SECONDS);

        assert_eq!(
            RateLimit::default()
                .with_limit(5000)
                .with_remaining(1)
                .with_used(4999)
                .with_reset(reset)
                .with_resource("graphql"),
            RateLimit {
                limit: Some(5000),
                remaining: Some(1),
                used: Some(4999),
                reset: Some(reset),
                resource: Some("graphql".to_string()),
            }
        );
    }
}
