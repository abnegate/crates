//! Failures raised by a completion provider.

use std::fmt;
use std::time::Duration;

use abnegate_secret::redact;
use thiserror::Error;

use crate::error::Error;
use crate::provider::exit_status::ExitStatus;

/// A provider failure.
///
/// Every variant renders the provider's own wording verbatim, after
/// [`redact`] has scrubbed any credential the process echoed back. A caller
/// classifies a run by matching that text, so a throttled request must still
/// read as a throttled request and a rejected key must still read as a
/// rejected key by the time it reaches the retry policy.
///
/// A variant may gain a field in a minor release, so a value is built with
/// its constructor ([`ProviderError::timeout`], [`ProviderError::agent`] and
/// the rest) and a pattern outside this crate ends in `..`:
///
/// ```compile_fail,E0639
/// use std::time::Duration;
///
/// use abnegate_llm::ProviderError;
///
/// let error = ProviderError::Timeout {
///     provider: "claude".to_string(),
///     timeout: Duration::from_secs(5),
/// };
/// # let _ = error;
/// ```
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ProviderError {
    /// An OpenAI-compatible endpoint failed, as `source` says.
    #[error("{provider}: {source}")]
    #[non_exhaustive]
    Http {
        provider: String,
        #[source]
        source: Error,
    },

    /// The agent command `executable` could not be started.
    #[error("{provider}: agent command {executable} could not be started: {reason}")]
    #[non_exhaustive]
    Unavailable {
        provider: String,
        executable: String,
        reason: String,
    },

    /// The agent command exited unsuccessfully.
    #[error("{provider}: agent command exited with status {status}: {message}")]
    #[non_exhaustive]
    Exit {
        provider: String,
        status: ExitStatus,
        message: String,
    },

    /// The agent command ran past its deadline and was stopped. The deadline
    /// renders in [`Duration`]'s `Debug` form, so 1,500 ms reads `1.5s` and
    /// 500 ms reads `500ms`.
    #[error("{provider}: agent command timed out after {timeout:?}")]
    #[non_exhaustive]
    Timeout { provider: String, timeout: Duration },

    /// The agent's output could not be read as an answer.
    #[error("{provider}: agent output could not be parsed: {message}")]
    #[non_exhaustive]
    Malformed { provider: String, message: String },

    /// The agent ran and reported a failure of its own.
    #[error("{provider}: {message}")]
    #[non_exhaustive]
    Agent { provider: String, message: String },

    /// The request never reached the provider, or its answer never arrived.
    #[error("network error: {detail}")]
    #[non_exhaustive]
    Network { detail: String },

    /// The provider answered with a failing HTTP `status`.
    #[error("API error (status {status}): {message}")]
    #[non_exhaustive]
    Api { status: u16, message: String },

    /// The provider's answer did not parse.
    #[error("parse error: {detail}")]
    #[non_exhaustive]
    Parse { detail: String },

    /// Reading or writing a local file failed.
    #[error("IO error: {detail}")]
    #[non_exhaustive]
    Io { detail: String },

    /// The provider is configured wrongly, such as with a missing key.
    #[error("configuration error: {detail}")]
    #[non_exhaustive]
    Config { detail: String },

    /// No provider is configured.
    #[error("no provider is configured")]
    Unconfigured,

    /// The provider does not offer the operation asked of it.
    #[error("unsupported operation: {detail}")]
    #[non_exhaustive]
    Unsupported { detail: String },

    /// Every one of `attempted` providers failed. `last` is the final
    /// failure, and it decides [`Self::provider`], [`Self::recoverable`] and
    /// [`Self::transient`].
    #[error("all {attempted} providers failed, last was {last}")]
    #[non_exhaustive]
    Exhausted {
        attempted: usize,
        last: Box<ProviderError>,
    },
}

const REJECTED: [u16; 4] = [400, 404, 413, 422];
const REQUEST_TIMEOUT: u16 = 408;
const TOO_MANY_REQUESTS: u16 = 429;
const FIRST_SERVER_ERROR: u16 = 500;

fn transient_status(status: u16) -> bool {
    status == REQUEST_TIMEOUT || status == TOO_MANY_REQUESTS || status >= FIRST_SERVER_ERROR
}

impl ProviderError {
    /// The failing provider's name, or `None` for a failure that belongs to no
    /// single provider.
    pub fn provider(&self) -> Option<&str> {
        match self {
            Self::Http { provider, .. }
            | Self::Unavailable { provider, .. }
            | Self::Exit { provider, .. }
            | Self::Timeout { provider, .. }
            | Self::Malformed { provider, .. }
            | Self::Agent { provider, .. } => Some(provider),
            Self::Network { .. }
            | Self::Api { .. }
            | Self::Parse { .. }
            | Self::Io { .. }
            | Self::Config { .. }
            | Self::Unconfigured
            | Self::Unsupported { .. } => None,
            Self::Exhausted { last, .. } => last.provider(),
        }
    }

    /// Whether trying the next provider in a chain can plausibly do better.
    ///
    /// A chain exists to survive one provider being throttled, offline, or not
    /// installed. It cannot rescue a request the caller built wrong, so a
    /// rejected request is not carried to a second provider that would reject
    /// it identically.
    pub fn recoverable(&self) -> bool {
        match self {
            Self::Http { source, .. } => {
                !matches!(source, Error::Api { status, .. } if REJECTED.contains(status))
            }
            Self::Api { status, .. } => !REJECTED.contains(status),
            Self::Unavailable { .. }
            | Self::Exit { .. }
            | Self::Timeout { .. }
            | Self::Malformed { .. }
            | Self::Agent { .. }
            | Self::Network { .. }
            | Self::Parse { .. }
            | Self::Io { .. } => true,
            Self::Config { .. } | Self::Unconfigured | Self::Unsupported { .. } => false,
            Self::Exhausted { last, .. } => last.recoverable(),
        }
    }

    /// An endpoint failure, with every message it carries redacted.
    pub fn http(provider: &str, source: Error) -> Self {
        Self::Http {
            provider: provider.to_string(),
            source: source.redacted(),
        }
    }

    /// Whether asking the same provider again, after a pause, can plausibly
    /// succeed: a throttle, a server-side failure, a dropped connection, a
    /// deadline, or an answer that did not parse.
    ///
    /// Narrower than [`Self::recoverable`]. A rejected key is worth taking to
    /// the next provider in a chain, which holds a different one, but asking
    /// the same provider again only gets the same refusal.
    pub fn transient(&self) -> bool {
        match self {
            Self::Http { source, .. } => match source {
                Error::Api { status, .. } => transient_status(*status),
                Error::Http(_) | Error::Stream(_) | Error::Timeout(_) => true,
                _ => false,
            },
            Self::Api { status, .. } => transient_status(*status),
            Self::Timeout { .. }
            | Self::Malformed { .. }
            | Self::Network { .. }
            | Self::Parse { .. } => true,
            Self::Unavailable { .. }
            | Self::Exit { .. }
            | Self::Agent { .. }
            | Self::Io { .. }
            | Self::Config { .. }
            | Self::Unconfigured
            | Self::Unsupported { .. } => false,
            Self::Exhausted { last, .. } => last.transient(),
        }
    }

    /// `provider`'s agent command ran past `timeout` and was stopped.
    pub fn timeout(provider: &str, timeout: Duration) -> Self {
        Self::Timeout {
            provider: provider.to_string(),
            timeout,
        }
    }

    /// `provider`'s agent command exited with `status`, saying `message`,
    /// which is redacted.
    pub fn exit(provider: &str, status: ExitStatus, message: &str) -> Self {
        Self::Exit {
            provider: provider.to_string(),
            status,
            message: redact(message).into_owned(),
        }
    }

    /// `provider`'s output could not be read, for the redacted `message`.
    pub fn malformed(provider: &str, message: impl fmt::Display) -> Self {
        Self::Malformed {
            provider: provider.to_string(),
            message: redact(&message.to_string()).into_owned(),
        }
    }

    /// `provider` reported the failure `message`, which is redacted.
    pub fn agent(provider: &str, message: &str) -> Self {
        Self::Agent {
            provider: provider.to_string(),
            message: redact(message).into_owned(),
        }
    }

    /// `provider`'s agent command `executable` could not be started, for the
    /// redacted `reason`.
    pub fn unavailable(provider: &str, executable: &str, reason: impl fmt::Display) -> Self {
        Self::Unavailable {
            provider: provider.to_string(),
            executable: executable.to_string(),
            reason: redact(&reason.to_string()).into_owned(),
        }
    }

    /// A [`Self::Network`] failure, with `detail` redacted.
    pub fn network(detail: impl fmt::Display) -> Self {
        Self::Network {
            detail: redact(&detail.to_string()).into_owned(),
        }
    }

    /// An HTTP `status` the provider answered with, and its redacted
    /// `message`.
    pub fn api(status: u16, message: impl fmt::Display) -> Self {
        Self::Api {
            status,
            message: redact(&message.to_string()).into_owned(),
        }
    }

    /// A [`Self::Parse`] failure, with `detail` redacted.
    pub fn parse(detail: impl fmt::Display) -> Self {
        Self::Parse {
            detail: redact(&detail.to_string()).into_owned(),
        }
    }

    /// A [`Self::Io`] failure, with `detail` redacted.
    pub fn io(detail: impl fmt::Display) -> Self {
        Self::Io {
            detail: redact(&detail.to_string()).into_owned(),
        }
    }

    /// A [`Self::Config`] failure, with `detail` redacted.
    pub fn config(detail: impl fmt::Display) -> Self {
        Self::Config {
            detail: redact(&detail.to_string()).into_owned(),
        }
    }

    /// A [`Self::Unsupported`] failure, with `detail` redacted.
    pub fn unsupported(detail: impl fmt::Display) -> Self {
        Self::Unsupported {
            detail: redact(&detail.to_string()).into_owned(),
        }
    }

    /// Every one of `attempted` providers failed, the last of them with
    /// `last`.
    pub fn exhausted(attempted: usize, last: ProviderError) -> Self {
        Self::Exhausted {
            attempted,
            last: Box::new(last),
        }
    }

    /// No provider behind `router` has the capabilities the request asked for.
    pub fn unsupported_route(router: &str) -> Self {
        Self::Unsupported {
            detail: format!("{router} has no configured provider with the required capabilities"),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::ProviderError;
    use crate::error::Error;
    use crate::provider::exit_status::ExitStatus;

    #[test]
    fn a_credential_echoed_by_the_agent_never_reaches_the_message() {
        let leaked = concat!(
            "Error: rejected key sk-ant-",
            "api03-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"
        );
        for error in [
            ProviderError::exit("claude", ExitStatus::Code(1), leaked),
            ProviderError::agent("claude", leaked),
            ProviderError::malformed("claude", leaked),
            ProviderError::unavailable("claude", "claude", leaked),
            ProviderError::network(leaked),
            ProviderError::api(500, leaked),
            ProviderError::parse(leaked),
            ProviderError::io(leaked),
            ProviderError::config(leaked),
            ProviderError::unsupported(leaked),
            ProviderError::http(
                "gateway",
                Error::Api {
                    status: 401,
                    message: leaked.to_string(),
                },
            ),
            ProviderError::http("gateway", Error::Stream(leaked.to_string())),
        ] {
            let rendered = format!("{error} {error:?}");
            assert!(
                !rendered.contains(concat!("sk-ant-", "api03-AAAA")),
                "credential survived in {rendered}"
            );
            assert!(
                rendered.contains("[REDACTED]"),
                "no redaction in {rendered}"
            );
        }
    }

    #[test]
    fn an_exhausted_chain_renders_the_last_failure_not_a_generic_one() {
        let error =
            ProviderError::exhausted(3, ProviderError::agent("codex", "429 rate limit reached"));

        let rendered = error.to_string();
        assert!(
            rendered.contains("rate limit"),
            "lost the cause: {rendered}"
        );
        assert!(rendered.contains("codex"), "lost the provider: {rendered}");
        assert!(rendered.starts_with("all 3 providers failed"), "{rendered}");
        assert_eq!(error.provider(), Some("codex"));
    }

    #[test]
    fn a_rejected_request_is_not_carried_to_the_next_provider() {
        for status in [400, 404, 413, 422] {
            let error = ProviderError::Http {
                provider: "gateway".to_string(),
                source: Error::Api {
                    status,
                    message: "invalid request".to_string(),
                },
            };
            assert!(!error.recoverable(), "status {status} should not fall over");
            assert!(
                !ProviderError::api(status, "invalid request").recoverable(),
                "status {status} should not fall over"
            );
        }

        for status in [429, 500, 502, 503] {
            let error = ProviderError::Http {
                provider: "gateway".to_string(),
                source: Error::Api {
                    status,
                    message: "upstream".to_string(),
                },
            };
            assert!(error.recoverable(), "status {status} should fall over");
            assert!(
                ProviderError::api(status, "upstream").recoverable(),
                "status {status} should fall over"
            );
        }
    }

    #[test]
    fn only_a_failure_that_can_clear_by_itself_is_transient() {
        for status in [408, 429, 500, 502, 503, 529] {
            assert!(ProviderError::api(status, "busy").transient(), "{status}");
            assert!(
                ProviderError::http(
                    "gateway",
                    Error::Api {
                        status,
                        message: "busy".into()
                    }
                )
                .transient(),
                "{status}"
            );
        }
        for status in [400, 401, 403, 404, 422] {
            assert!(!ProviderError::api(status, "no").transient(), "{status}");
        }
        assert!(ProviderError::network("reset").transient());
        assert!(
            ProviderError::http("gateway", Error::Timeout(std::time::Duration::from_secs(1)))
                .transient()
        );
        assert!(!ProviderError::config("missing key").transient());
        assert!(!ProviderError::unavailable("claude", "claude", "not found").transient());
    }

    #[test]
    fn a_routing_failure_belongs_to_no_provider_and_is_not_retried() {
        for error in [
            ProviderError::Unconfigured,
            ProviderError::unsupported_route("router"),
            ProviderError::config("missing key"),
        ] {
            assert_eq!(error.provider(), None);
            assert!(!error.recoverable());
        }
    }

    #[test]
    fn a_transport_failure_belongs_to_no_provider_but_is_retried() {
        for error in [
            ProviderError::network("connection refused"),
            ProviderError::parse("invalid json"),
            ProviderError::io("file not found"),
        ] {
            assert_eq!(error.provider(), None);
            assert!(error.recoverable());
        }
    }

    #[test]
    fn every_call_failure_says_what_went_wrong() {
        assert_eq!(
            ProviderError::network("connection refused").to_string(),
            "network error: connection refused"
        );
        assert_eq!(
            ProviderError::api(429, "rate limited").to_string(),
            "API error (status 429): rate limited"
        );
        assert_eq!(
            ProviderError::parse("invalid json").to_string(),
            "parse error: invalid json"
        );
        assert_eq!(
            ProviderError::config("missing key").to_string(),
            "configuration error: missing key"
        );
        assert_eq!(
            ProviderError::unsupported("feature X").to_string(),
            "unsupported operation: feature X"
        );
        assert_eq!(
            ProviderError::io("file not found").to_string(),
            "IO error: file not found"
        );
    }

    #[test]
    fn a_timeout_keeps_the_fraction_of_a_second_it_waited() {
        for (timeout, rendered) in [
            (
                Duration::from_millis(1_500),
                "claude: agent command timed out after 1.5s",
            ),
            (
                Duration::from_millis(500),
                "claude: agent command timed out after 500ms",
            ),
            (
                Duration::from_secs(600),
                "claude: agent command timed out after 600s",
            ),
        ] {
            let error = ProviderError::timeout("claude", timeout);

            assert_eq!(error.to_string(), rendered);
            assert!(
                matches!(&error, ProviderError::Timeout { timeout: kept, .. } if *kept == timeout),
                "{error:?}"
            );
        }
    }

    #[test]
    fn a_routing_failure_names_the_router_that_could_not_place_the_request() {
        let rendered = ProviderError::unsupported_route("strict").to_string();
        assert!(rendered.contains("strict"), "lost the router: {rendered}");
        assert!(
            rendered.contains("required capabilities"),
            "lost the reason: {rendered}"
        );
    }
}
