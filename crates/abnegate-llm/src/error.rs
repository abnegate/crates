use abnegate_secret::redact;
use thiserror::Error;

/// A failure reaching, or reading, an OpenAI-compatible endpoint.
///
/// Every message an endpoint sends back has had [`abnegate_secret::redact`]
/// applied before it lands here, and a transport failure carries no URL, so a
/// key echoed in a rejection body or carried in a query string never reaches
/// a log line through this type.
#[derive(Debug, Error)]
pub enum LlmError {
    #[error("HTTP error: {0}")]
    Http(reqwest::Error),
    #[error("API error: {status} - {message}")]
    Api { status: u16, message: String },
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Stream error: {0}")]
    Stream(String),
    #[error("Invalid configuration: {0}")]
    InvalidConfig(String),
}

impl From<reqwest::Error> for LlmError {
    fn from(error: reqwest::Error) -> Self {
        Self::Http(error.without_url())
    }
}

impl LlmError {
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
    use super::LlmError;

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
                LlmError::Api {
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
                !LlmError::Api {
                    status,
                    message: message.into()
                }
                .unsupported_tools()
            );
        }
        assert!(!LlmError::Stream("does not support tools".into()).unsupported_tools());
    }

    #[test]
    fn an_api_failure_renders_its_status_and_body() {
        let error = LlmError::Api {
            status: 401,
            message: "Unauthorized".to_string(),
        };

        let display = error.to_string();
        assert!(display.contains("401"));
        assert!(display.contains("Unauthorized"));
        assert!(display.contains("API error"));
    }

    #[test]
    fn a_stream_failure_renders_its_cause() {
        let error = LlmError::Stream("Connection reset".to_string());
        let display = error.to_string();

        assert!(display.contains("Stream error"));
        assert!(display.contains("Connection reset"));
    }

    #[test]
    fn a_serde_failure_converts_into_a_json_error() {
        let failure = serde_json::from_str::<serde_json::Value>("invalid json").unwrap_err();

        let error: LlmError = failure.into();
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
            let error = LlmError::Api {
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
        let error = LlmError::Api {
            status: 500,
            message: "Server error".to_string(),
        };
        let debug = format!("{error:?}");

        assert!(debug.contains("Api"));
        assert!(debug.contains("500"));
    }

    #[test]
    fn an_empty_stream_message_still_names_the_kind() {
        let error = LlmError::Stream(String::new());
        assert!(error.to_string().contains("Stream error"));
    }

    #[test]
    fn an_empty_api_body_still_names_the_status() {
        let error = LlmError::Api {
            status: 500,
            message: String::new(),
        };
        let display = error.to_string();
        assert!(display.contains("500"));
        assert!(display.contains("API error"));
    }
}
