use std::fmt;
use std::time::Duration;

use abnegate_secret::redact;
use thiserror::Error;

use crate::provider::Limit;
use crate::provider::ProviderError;

/// A failure reaching, or reading, an OpenAI-compatible endpoint, or a
/// [`ProviderError`] carried into a chat stream.
///
/// Every message an endpoint sends back has had [`abnegate_secret::redact`]
/// applied before it lands here, and a transport failure carries no URL, so a
/// key echoed in a rejection body or carried in a query string never reaches
/// a log line through this type.
///
/// [`Error::Api`] may gain a field in a minor release, so a value is built
/// with [`Error::api`] and a pattern outside this crate ends in `..`:
///
/// ```compile_fail,E0639
/// let error = abnegate_llm::Error::Api {
///     status: 429,
///     message: "slow down".to_string(),
/// };
/// # let _ = error;
/// ```
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
    /// The request could not be sent, or its answer could not be read.
    #[error("HTTP error: {0}")]
    Http(reqwest::Error),
    /// The endpoint answered with a failing HTTP `status`.
    #[error("API error: {status} - {message}")]
    #[non_exhaustive]
    Api {
        /// The HTTP status code.
        status: u16,
        /// The endpoint's explanation, from its response body.
        message: String,
    },
    /// A body did not serialise, or did not parse.
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    /// A streamed answer broke off or could not be read.
    #[error("Stream error: {0}")]
    Stream(String),
    /// The client is configured wrongly.
    #[error("Invalid configuration: {0}")]
    InvalidConfig(String),
    /// The endpoint sent nothing for as long as the client waits.
    #[error("the endpoint sent nothing for {0:?}")]
    Timeout(Duration),
    /// A coding agent's own report of what went wrong, in its own words, so
    /// the reader sees "Not logged in" rather than a status code.
    #[error("{0}")]
    Agent(String),
    /// A usage limit refused a coding agent's turn. It renders exactly as the
    /// [`Error::Agent`] the same refusal would be, so anything that reads a
    /// failure's text reads it unchanged.
    #[error("{provider}: {}", .limit.message)]
    #[non_exhaustive]
    Limited {
        /// The provider that failed.
        provider: String,
        /// The refusal.
        limit: Box<Limit>,
    },
}

impl From<reqwest::Error> for Error {
    fn from(error: reqwest::Error) -> Self {
        Self::Http(error.without_url())
    }
}

/// A limit stays [`Error::Limited`], an endpoint failure becomes the error it
/// carries and an HTTP status stays [`Error::Api`], an exhausted chain
/// becomes its last failure, and anything else becomes [`Error::Agent`] in the
/// provider error's own words.
impl From<ProviderError> for Error {
    fn from(error: ProviderError) -> Self {
        match error {
            ProviderError::Limited { provider, limit } => Self::limited(&provider, *limit),
            ProviderError::Http { source, .. } => source.redacted(),
            ProviderError::Api { status, message } => Self::api(status, message),
            ProviderError::Exhausted { last, .. } => Self::from(*last),
            error => Self::agent(error),
        }
    }
}

impl Error {
    /// A coding agent's report of a failure, which is redacted.
    pub fn agent(message: impl fmt::Display) -> Self {
        Self::Agent(redact(&message.to_string()).into_owned())
    }

    /// A usage limit refused `provider`'s turn, with the limit's message
    /// redacted.
    pub fn limited(provider: &str, limit: Limit) -> Self {
        let message = redact(&limit.message).into_owned();
        Self::Limited {
            provider: provider.to_string(),
            limit: Box::new(limit.with_message(message)),
        }
    }

    /// The endpoint answered with `status`, saying `message`, which is
    /// redacted.
    pub fn api(status: u16, message: impl fmt::Display) -> Self {
        Self::Api {
            status,
            message: redact(&message.to_string()).into_owned(),
        }
    }

    /// The same failure with [`redact`] applied to every message it carries,
    /// for a value that did not come through [`crate::LlmClient`].
    pub fn redacted(self) -> Self {
        match self {
            Self::Api { status, message } => Self::Api {
                status,
                message: redact(&message).into_owned(),
            },
            Self::Stream(message) => Self::Stream(redact(&message).into_owned()),
            Self::InvalidConfig(message) => Self::InvalidConfig(redact(&message).into_owned()),
            Self::Agent(message) => Self::agent(message),
            Self::Limited { provider, limit } => Self::limited(&provider, *limit),
            Self::Http(error) => Self::Http(error.without_url()),
            other => other,
        }
    }

    /// Whether the provider explicitly rejected this model's tool capability.
    ///
    /// A proxy in front of Ollama wraps its capability rejection as an
    /// `APIConnectionError`, which arrives as a 500 rather than a 400.
    pub fn unsupported_tools(&self) -> bool {
        let Self::Api {
            status: 400 | 500,
            message,
        } = self
        else {
            return false;
        };
        let Ok(error) = serde_json::from_str::<serde_json::Value>(message) else {
            return false;
        };
        error
            .pointer("/error/message")
            .or_else(|| error.get("error"))
            .and_then(serde_json::Value::as_str)
            .is_some_and(|message| message.contains("does not support tools"))
    }
}

#[cfg(test)]
mod tests {
    use chrono::DateTime;

    use super::Error;
    use crate::provider::Limit;
    use crate::provider::ProviderError;

    const SESSION_LIMIT: &str =
        "rate limit reached (five_hour, rejected): You've hit your session limit · resets 5pm";

    fn session_limit() -> Limit {
        Limit::new(SESSION_LIMIT)
            .with_resets_at(DateTime::from_timestamp(1_790_208_000, 0).expect("a valid timestamp"))
    }

    #[test]
    fn a_limit_error_reads_as_the_agent_error_it_replaces() {
        for message in [
            SESSION_LIMIT.to_string(),
            "The signed-in Claude account cannot spend usage credits on this model: Fable 5.1 \
             requires usage credits. Switch to another model to continue."
                .to_string(),
            "You have hit your usage limit.\n\nThe end of the agent's stderr:\nwarning: the plan \
             is spent"
                .to_string(),
        ] {
            let limit = session_limit().with_message(message.clone());
            let before = Error::Agent(ProviderError::agent("claude", &message).to_string());
            let provider = ProviderError::limited("claude", limit.clone());

            assert_eq!(provider.to_string(), before.to_string());
            assert_eq!(Error::from(provider).to_string(), before.to_string());
            assert_eq!(
                Error::limited("claude", limit.clone()).to_string(),
                before.to_string()
            );
            assert_eq!(
                format!("Stream error: {}", Error::limited("claude", limit)),
                format!("Stream error: claude: {message}")
            );
        }
    }

    #[test]
    fn a_limited_provider_failure_keeps_its_limit_and_any_other_reads_as_the_agent() {
        let Error::Limited { provider, limit } =
            Error::from(ProviderError::limited("claude", session_limit()))
        else {
            panic!("a limit lost its variant");
        };
        assert_eq!(provider, "claude");
        assert_eq!(*limit, session_limit());

        let refused = ProviderError::agent("codex", "Not logged in · Please run /login");
        let rendered = refused.to_string();
        let Error::Agent(message) = Error::from(refused) else {
            panic!("an agent failure lost its wording");
        };
        assert_eq!(message, rendered);
    }

    #[test]
    fn an_endpoint_failure_keeps_its_status_through_the_conversion() {
        let from_http = Error::from(ProviderError::http("gateway", Error::api(429, "slow down")));
        assert!(
            matches!(&from_http, Error::Api { status: 429, message } if message == "slow down"),
            "{from_http:?}"
        );

        let from_api = Error::from(ProviderError::api(503, "overloaded"));
        assert!(
            matches!(&from_api, Error::Api { status: 503, message } if message == "overloaded"),
            "{from_api:?}"
        );

        let timeout = std::time::Duration::from_secs(5);
        let from_timeout = Error::from(ProviderError::http("gateway", Error::Timeout(timeout)));
        assert!(
            matches!(from_timeout, Error::Timeout(kept) if kept == timeout),
            "{from_timeout:?}"
        );
    }

    #[test]
    fn an_exhausted_chain_converts_as_its_last_failure() {
        let limited = Error::from(ProviderError::exhausted(
            2,
            ProviderError::limited("claude", session_limit()),
        ));
        let Error::Limited { provider, limit } = limited else {
            panic!("a limit behind a chain lost its variant: {limited:?}");
        };
        assert_eq!(provider, "claude");
        assert_eq!(*limit, session_limit());

        let nested = Error::from(ProviderError::exhausted(
            3,
            ProviderError::exhausted(2, ProviderError::http("gateway", Error::api(401, "no"))),
        ));
        assert!(
            matches!(nested, Error::Api { status: 401, .. }),
            "{nested:?}"
        );

        let agent = ProviderError::agent("codex", "Not logged in");
        let rendered = agent.to_string();
        let Error::Agent(message) = Error::from(ProviderError::exhausted(2, agent)) else {
            panic!("an agent failure behind a chain lost its wording");
        };
        assert_eq!(message, rendered);
    }

    #[test]
    fn an_agent_failure_never_carries_a_credential() {
        let leaked = concat!(
            "Error: rejected key sk-ant-",
            "api03-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"
        );
        for error in [
            Error::agent(leaked),
            Error::limited("claude", Limit::new(leaked)),
            Error::Agent(leaked.to_string()).redacted(),
            Error::Limited {
                provider: "claude".to_string(),
                limit: Box::new(Limit::new(leaked)),
            }
            .redacted(),
            Error::from(ProviderError::agent("claude", leaked)),
            Error::from(ProviderError::Http {
                provider: "gateway".to_string(),
                source: Error::Stream(leaked.to_string()),
            }),
            Error::from(ProviderError::Api {
                status: 401,
                message: leaked.to_string(),
            }),
        ] {
            let rendered = format!("{error} {error:?}");
            assert!(
                !rendered.contains(concat!("sk-ant-", "api03-AAAA")),
                "credential survived in {rendered}"
            );
            assert!(rendered.contains("[REDACTED]"), "{rendered}");
        }
    }

    #[test]
    fn classifies_explicit_tool_capability_rejections() {
        for (status, message) in [
            (400, r#"{"error":"llava:7b does not support tools"}"#),
            (
                500,
                r#"{"error":{"message":"Ollama_chatException - llava:7b does not support tools"}}"#,
            ),
        ] {
            assert!(
                Error::Api {
                    status,
                    message: message.into()
                }
                .unsupported_tools()
            );
        }
    }

    #[test]
    fn preserves_authentication_transport_and_schema_failures() {
        for (status, message) in [
            (401, r#"{"error":{"message":"does not support tools"}}"#),
            (403, r#"{"error":{"message":"does not support tools"}}"#),
            (429, r#"{"error":{"message":"does not support tools"}}"#),
            (500, r#"{"error":{"message":"Connection refused"}}"#),
            (400, r#"{"error":{"message":"Invalid tool schema"}}"#),
            (500, "does not support tools"),
        ] {
            assert!(
                !Error::Api {
                    status,
                    message: message.into()
                }
                .unsupported_tools()
            );
        }
        assert!(!Error::Stream("does not support tools".into()).unsupported_tools());
    }

    #[test]
    fn an_api_failure_renders_its_status_and_body() {
        let error = Error::Api {
            status: 401,
            message: "Unauthorized".to_string(),
        };

        let display = error.to_string();
        assert!(display.contains("401"));
        assert!(display.contains("Unauthorized"));
        assert!(display.contains("API error"));
    }

    #[test]
    fn an_api_failure_a_caller_builds_is_redacted() {
        let leaked = concat!(
            "rejected key sk-ant-",
            "api03-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"
        );

        let error = Error::api(401, leaked);

        let rendered = format!("{error} {error:?}");
        assert!(
            !rendered.contains(concat!("sk-ant-", "api03-AAAA")),
            "credential survived in {rendered}"
        );
        assert!(rendered.contains("[REDACTED]"), "{rendered}");
        assert!(matches!(error, Error::Api { status: 401, .. }));
    }

    #[test]
    fn a_stream_failure_renders_its_cause() {
        let error = Error::Stream("Connection reset".to_string());
        let display = error.to_string();

        assert!(display.contains("Stream error"));
        assert!(display.contains("Connection reset"));
    }

    #[test]
    fn a_serde_failure_converts_into_a_json_error() {
        let failure = serde_json::from_str::<serde_json::Value>("invalid json").unwrap_err();

        let error: Error = failure.into();
        assert!(error.to_string().contains("JSON error"));
    }

    #[test]
    fn every_api_status_renders_with_its_body() {
        for (status, message) in [
            (400, "Bad Request"),
            (401, "Unauthorized"),
            (403, "Forbidden"),
            (404, "Not Found"),
            (429, "Rate Limited"),
            (500, "Internal Server Error"),
            (503, "Service Unavailable"),
        ] {
            let error = Error::Api {
                status,
                message: message.to_string(),
            };
            let display = error.to_string();
            assert!(display.contains(&status.to_string()));
            assert!(display.contains(message));
        }
    }

    #[test]
    fn debug_names_the_variant_and_the_status() {
        let error = Error::Api {
            status: 500,
            message: "Server error".to_string(),
        };
        let debug = format!("{error:?}");

        assert!(debug.contains("Api"));
        assert!(debug.contains("500"));
    }

    #[test]
    fn an_empty_stream_message_still_names_the_kind() {
        let error = Error::Stream(String::new());
        assert!(error.to_string().contains("Stream error"));
    }

    #[test]
    fn an_empty_api_body_still_names_the_status() {
        let error = Error::Api {
            status: 500,
            message: String::new(),
        };
        let display = error.to_string();
        assert!(display.contains("500"));
        assert!(display.contains("API error"));
    }
}
