use serde_json::Value;

use crate::error::Error;
use crate::provider::ExitStatus;
use crate::provider::ProviderError;

const API_ERROR_PREFIX: &str = "API Error: ";
const UNDESCRIBED: &str = "the claude CLI reported a failure without describing it";

/// A failed turn, as the Claude Code CLI describes it.
///
/// With `--output-format json` the CLI reports a failure on stdout, in the
/// same envelope as an answer but with `is_error` set. An API refusal comes
/// as `result` text beginning `API Error: <status>`, usually with the status
/// under `api_error_status` too; any other failure comes as `result` text or
/// a list of `errors`.
#[derive(Debug)]
pub(crate) struct Failure {
    message: String,
    status: Option<u16>,
}

impl Failure {
    /// The failure `envelope` reports, when its `is_error` is set.
    pub(crate) fn reported(envelope: &Value) -> Option<Self> {
        let failed = envelope
            .get("is_error")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        failed.then(|| {
            Self::described(envelope).unwrap_or_else(|| Self {
                message: UNDESCRIBED.to_string(),
                status: None,
            })
        })
    }

    /// Why a run that exited unsuccessfully failed: the envelope's account
    /// when stdout holds one, and the run's diagnostics otherwise.
    pub(crate) fn exited(stdout: &[u8], stderr: &[u8]) -> Self {
        serde_json::from_slice::<Value>(stdout)
            .ok()
            .and_then(|envelope| Self::described(&envelope))
            .unwrap_or_else(|| Self {
                message: String::from_utf8_lossy(stderr).into_owned(),
                status: None,
            })
    }

    /// The error to report for a run that ended with `exit`.
    ///
    /// A failure with an API status is reported as that API failure, so a
    /// throttle or an overload is retried like one over HTTP.
    pub(crate) fn into_error(self, provider: &str, exit: ExitStatus) -> ProviderError {
        match (self.status, exit) {
            (Some(status), _) => ProviderError::http(
                provider,
                Error::Api {
                    status,
                    message: self.message,
                },
            ),
            (None, ExitStatus::Code(0)) => ProviderError::agent(provider, &self.message),
            (None, exit) => ProviderError::exit(provider, exit, &self.message),
        }
    }

    fn described(envelope: &Value) -> Option<Self> {
        let message = envelope
            .get("result")
            .and_then(Value::as_str)
            .filter(|result| !result.trim().is_empty())
            .map(str::to_string)
            .or_else(|| errors(envelope))?;
        let status = envelope
            .get("api_error_status")
            .and_then(Value::as_u64)
            .and_then(|status| u16::try_from(status).ok())
            .or_else(|| status_in(&message));
        Some(Self { message, status })
    }
}

fn errors(envelope: &Value) -> Option<String> {
    let errors: Vec<&str> = envelope
        .get("errors")?
        .as_array()?
        .iter()
        .filter_map(Value::as_str)
        .collect();
    (!errors.is_empty()).then(|| errors.join("; "))
}

/// The status in `API Error: <status> ...`.
fn status_in(message: &str) -> Option<u16> {
    let rest = message.trim_start().strip_prefix(API_ERROR_PREFIX)?;
    let digits = rest
        .split(|character: char| !character.is_ascii_digit())
        .next()?;
    if digits.len() != 3 {
        return None;
    }
    digits.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_envelope_that_is_not_an_error_reports_nothing() {
        let envelope = serde_json::json!({ "is_error": false, "result": "hello" });
        assert!(Failure::reported(&envelope).is_none());
        assert!(Failure::reported(&serde_json::json!({ "result": "hello" })).is_none());
    }

    #[test]
    fn an_error_envelope_that_says_nothing_still_fails() {
        let failure = Failure::reported(&serde_json::json!({ "is_error": true })).unwrap();
        assert_eq!(failure.message, UNDESCRIBED);
        assert_eq!(failure.status, None);
    }

    #[test]
    fn the_status_field_wins_over_the_text() {
        let failure = Failure::reported(&serde_json::json!({
            "is_error": true,
            "api_error_status": 503,
            "result": "API Error: 500 upstream"
        }))
        .unwrap();
        assert_eq!(failure.status, Some(503));
    }

    #[test]
    fn a_status_is_only_read_from_an_api_error() {
        assert_eq!(status_in("API Error: 429 {\"type\":\"error\"}"), Some(429));
        assert_eq!(status_in("API Error: 401 Invalid API key"), Some(401));
        assert_eq!(status_in("API Error: Request was aborted."), None);
        assert_eq!(status_in("API Error: 4290 nope"), None);
        assert_eq!(status_in("Error: 429"), None);
    }

    #[test]
    fn errors_are_joined_when_there_is_no_result() {
        let failure = Failure::reported(&serde_json::json!({
            "is_error": true,
            "subtype": "error_during_execution",
            "errors": ["first", "second"]
        }))
        .unwrap();
        assert_eq!(failure.message, "first; second");
    }

    #[test]
    fn diagnostics_stand_in_for_an_envelope_that_is_not_there() {
        let failure = Failure::exited(b"not json", b"not signed in");
        assert_eq!(failure.message, "not signed in");
        assert_eq!(failure.status, None);
    }

    #[test]
    fn a_failure_without_a_status_is_an_exit_or_an_agent_error() {
        let failure = || Failure {
            message: "no".into(),
            status: None,
        };
        assert!(matches!(
            failure().into_error("anthropic", ExitStatus::Code(2)),
            ProviderError::Exit {
                status: ExitStatus::Code(2),
                ..
            }
        ));
        assert!(matches!(
            failure().into_error("anthropic", ExitStatus::Code(0)),
            ProviderError::Agent { .. }
        ));
    }
}
