use std::fmt;
use std::time::Duration;

use abnegate_secret::redact;
use thiserror::Error;

/// A failure reaching, or reading, an OpenAI-compatible endpoint.
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
    Api { status: u16, message: String },
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
}

impl From<reqwest::Error> for Error {
    fn from(error: reqwest::Error) -> Self {
        Self::Http(error.without_url())
    }
}

impl Error {
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
    use super::Error;

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
