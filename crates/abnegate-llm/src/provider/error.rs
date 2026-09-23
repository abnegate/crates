//! Failures raised by a completion provider.

use std::fmt;

use abnegate_secret::redact;
use thiserror::Error;

use crate::error::LlmError;

/// A provider failure.
///
/// Every variant renders the provider's own wording verbatim, after
/// [`redact`] has scrubbed any credential the process echoed back. A caller
/// classifies a run by matching that text, so a throttled request must still
/// read as a throttled request and a rejected key must still read as a
/// rejected key by the time it reaches the retry policy.
#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("{provider}: {source}")]
    Http {
        provider: String,
        #[source]
        source: LlmError,
    },

    #[error("{provider}: agent command {executable} could not be started: {reason}")]
    Unavailable {
        provider: String,
        executable: String,
        reason: String,
    },

    #[error("{provider}: agent command exited with status {status}: {message}")]
    Exit {
        provider: String,
        status: ExitStatus,
        message: String,
    },

    #[error("{provider}: agent command timed out after {seconds} seconds")]
    Timeout { provider: String, seconds: u64 },

    #[error("{provider}: agent output could not be parsed: {message}")]
    Malformed { provider: String, message: String },

    #[error("{provider}: {message}")]
    Agent { provider: String, message: String },

    #[error("no provider is configured")]
    Unconfigured,

    #[error("{router}: no configured provider has the required capabilities")]
    Unsupported { router: String },

    #[error("all {attempted} providers failed, last was {last}")]
    Exhausted {
        attempted: usize,
        last: Box<ProviderError>,
    },
}

impl ProviderError {
    /// The failing provider's name, or `None` for a routing failure that
    /// belongs to no single provider.
    pub fn provider(&self) -> Option<&str> {
        match self {
            Self::Http { provider, .. }
            | Self::Unavailable { provider, .. }
            | Self::Exit { provider, .. }
            | Self::Timeout { provider, .. }
            | Self::Malformed { provider, .. }
            | Self::Agent { provider, .. } => Some(provider),
            Self::Unconfigured | Self::Unsupported { .. } => None,
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
            Self::Http { source, .. } => !matches!(
                source,
                LlmError::Api {
                    status: 400 | 404 | 413 | 422,
                    ..
                }
            ),
            Self::Unavailable { .. }
            | Self::Exit { .. }
            | Self::Timeout { .. }
            | Self::Malformed { .. }
            | Self::Agent { .. } => true,
            Self::Unconfigured | Self::Unsupported { .. } => false,
            Self::Exhausted { last, .. } => last.recoverable(),
        }
    }

    pub fn exit(provider: &str, status: ExitStatus, message: &str) -> Self {
        Self::Exit {
            provider: provider.to_string(),
            status,
            message: redact(message).into_owned(),
        }
    }

    pub fn malformed(provider: &str, message: impl fmt::Display) -> Self {
        Self::Malformed {
            provider: provider.to_string(),
            message: redact(&message.to_string()).into_owned(),
        }
    }

    pub fn agent(provider: &str, message: &str) -> Self {
        Self::Agent {
            provider: provider.to_string(),
            message: redact(message).into_owned(),
        }
    }

    pub fn unavailable(provider: &str, executable: &str, reason: impl fmt::Display) -> Self {
        Self::Unavailable {
            provider: provider.to_string(),
            executable: executable.to_string(),
            reason: redact(&reason.to_string()).into_owned(),
        }
    }
}

/// How a child process ended.
///
/// A signalled process has no exit code, and reporting one as `-1` loses the
/// difference between a crash and a command that genuinely returned `-1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitStatus {
    Code(i32),
    Signalled,
}

impl fmt::Display for ExitStatus {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Code(code) => write!(formatter, "{code}"),
            Self::Signalled => formatter.write_str("signal"),
        }
    }
}

impl From<std::process::ExitStatus> for ExitStatus {
    fn from(status: std::process::ExitStatus) -> Self {
        status.code().map_or(Self::Signalled, Self::Code)
    }
}

#[cfg(test)]
mod tests {
    use super::ExitStatus;
    use super::ProviderError;
    use crate::error::LlmError;

    #[test]
    fn a_credential_echoed_by_the_agent_never_reaches_the_message() {
        let leaked = "Error: rejected key sk-ant-api03-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
        for error in [
            ProviderError::exit("claude", ExitStatus::Code(1), leaked),
            ProviderError::agent("claude", leaked),
            ProviderError::malformed("claude", leaked),
            ProviderError::unavailable("claude", "claude", leaked),
        ] {
            let rendered = format!("{error} {error:?}");
            assert!(
                !rendered.contains("sk-ant-api03-AAAA"),
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
        let error = ProviderError::Exhausted {
            attempted: 3,
            last: Box::new(ProviderError::agent("codex", "429 rate limit reached")),
        };

        let rendered = error.to_string();
        assert!(
            rendered.contains("rate limit"),
            "lost the cause: {rendered}"
        );
        assert!(rendered.contains("codex"), "lost the provider: {rendered}");
        assert_eq!(error.provider(), Some("codex"));
    }

    #[test]
    fn a_rejected_request_is_not_carried_to_the_next_provider() {
        for status in [400, 404, 413, 422] {
            let error = ProviderError::Http {
                provider: "gateway".to_string(),
                source: LlmError::Api {
                    status,
                    message: "invalid request".to_string(),
                },
            };
            assert!(!error.recoverable(), "status {status} should not fall over");
        }

        for status in [429, 500, 502, 503] {
            let error = ProviderError::Http {
                provider: "gateway".to_string(),
                source: LlmError::Api {
                    status,
                    message: "upstream".to_string(),
                },
            };
            assert!(error.recoverable(), "status {status} should fall over");
        }
    }

    #[test]
    fn a_routing_failure_belongs_to_no_provider_and_is_not_retried() {
        for error in [
            ProviderError::Unconfigured,
            ProviderError::Unsupported {
                router: "router".to_string(),
            },
        ] {
            assert_eq!(error.provider(), None);
            assert!(!error.recoverable());
        }
    }

    #[test]
    fn exit_status_keeps_a_signal_distinct_from_a_code() {
        assert_eq!(ExitStatus::Code(2).to_string(), "2");
        assert_eq!(ExitStatus::Signalled.to_string(), "signal");
    }
}
