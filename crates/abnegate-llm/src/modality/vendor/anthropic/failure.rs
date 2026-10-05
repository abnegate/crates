use serde_json::Value;

use crate::error::Error;
use crate::modality::vendor::anthropic::cause::Cause;
use crate::provider::ExitStatus;
use crate::provider::ProviderError;

const UNDESCRIBED: &str = "the claude CLI reported a failure without describing it";

/// A failed turn, as the Claude Code CLI describes it.
///
/// With `--output-format json` the CLI reports a failure on stdout, in the
/// same envelope as an answer but with `is_error` set. An API failure comes
/// as `result` text beginning `API Error: `, followed by its status, the
/// API's JSON error body, or the CLI's own wording such as
/// `Connection error.`, usually with the status under `api_error_status` too;
/// any other failure comes as `result` text or a list of `errors`.
///
/// Only those `API Error: ` lines are read for what went wrong, whether they
/// come in `result`, in an entry of `errors`, or on stderr when stdout holds
/// no failed envelope. Everything else the CLI can relay, such as a hook's or
/// a tool's output and the telemetry noise on stderr, is reported but never
/// classified.
#[derive(Debug)]
pub(crate) struct Failure {
    message: String,
    cause: Cause,
}

impl Failure {
    /// The failure `envelope` reports, when its `is_error` is set.
    pub(crate) fn reported(envelope: &Value) -> Option<Self> {
        failed(envelope).then(|| Self::described(envelope).unwrap_or_else(Self::undescribed))
    }

    /// Why a run that exited unsuccessfully failed: the envelope's account
    /// when stdout holds one that reports a failure, and the run's
    /// diagnostics otherwise.
    pub(crate) fn exited(stdout: &[u8], stderr: &[u8]) -> Self {
        serde_json::from_slice::<Value>(stdout)
            .ok()
            .filter(failed)
            .and_then(|envelope| Self::described(&envelope))
            .map(|failure| failure.told(stderr))
            .unwrap_or_else(|| Self::diagnosed(stderr))
    }

    /// The error to report for a run that ended with `exit`.
    ///
    /// The CLI's API failures are reported as the messages API route reports
    /// the same failures, so both routes retry the same ones: one with a
    /// status, an error type or overload wording as that API failure, and a
    /// dropped connection or a timeout as [`ProviderError::Network`], which
    /// like the API route's carries no provider name. The CLI has already
    /// retried such a failure itself before reporting it, so a caller's
    /// retries, such as [`AiClient`](crate::modality::AiClient)'s bounded
    /// `retries`, come on top of the CLI's own.
    pub(crate) fn into_error(self, provider: &str, exit: ExitStatus) -> ProviderError {
        match (self.cause, exit) {
            (Cause::Status(status), _) => ProviderError::http(
                provider,
                Error::Api {
                    status,
                    message: self.message,
                },
            ),
            (Cause::Network, _) => ProviderError::network(self.message),
            (Cause::Unclassified, ExitStatus::Code(0)) => {
                ProviderError::agent(provider, &self.message)
            }
            (Cause::Unclassified, exit) => ProviderError::exit(provider, exit, &self.message),
        }
    }

    /// This failure, worded by the run's diagnostics when its envelope gave
    /// a status but no words of its own.
    fn told(mut self, stderr: &[u8]) -> Self {
        let diagnostics = String::from_utf8_lossy(stderr);
        if self.message == UNDESCRIBED && !diagnostics.trim().is_empty() {
            self.message = diagnostics.trim().to_string();
        }
        self
    }

    fn undescribed() -> Self {
        Self {
            message: UNDESCRIBED.into(),
            cause: Cause::Unclassified,
        }
    }

    fn diagnosed(stderr: &[u8]) -> Self {
        let diagnostics = String::from_utf8_lossy(stderr);
        let cause = Cause::read(diagnostics.lines());
        Self {
            message: diagnostics.into_owned(),
            cause,
        }
    }

    fn described(envelope: &Value) -> Option<Self> {
        let status = envelope
            .get("api_error_status")
            .and_then(Value::as_u64)
            .and_then(|status| u16::try_from(status).ok());
        if let Some(result) = envelope
            .get("result")
            .and_then(Value::as_str)
            .filter(|result| !result.trim().is_empty())
        {
            return Some(Self::classified(result.to_string(), status, result.lines()));
        }
        match errors(envelope) {
            Some(errors) => Some(Self::classified(
                errors.join("; "),
                status,
                errors.iter().flat_map(|error| error.lines()),
            )),
            None => status.map(|status| Self {
                message: UNDESCRIBED.into(),
                cause: Cause::Status(status),
            }),
        }
    }

    /// `message`, caused by `status` when the envelope names one, and by
    /// what the `API Error: ` lines among `lines` say otherwise.
    fn classified<'a>(
        message: String,
        status: Option<u16>,
        lines: impl IntoIterator<Item = &'a str>,
    ) -> Self {
        let cause = status.map_or_else(|| Cause::read(lines), Cause::Status);
        Self { message, cause }
    }
}

fn failed(envelope: &Value) -> bool {
    envelope
        .get("is_error")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn errors(envelope: &Value) -> Option<Vec<&str>> {
    let errors: Vec<&str> = envelope
        .get("errors")?
        .as_array()?
        .iter()
        .filter_map(Value::as_str)
        .collect();
    (!errors.is_empty()).then_some(errors)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modality::vendor::anthropic::CLI_NAME;
    use crate::modality::vendor::anthropic::cause::OVERLOADED;

    fn failure_from(message: &str) -> Failure {
        Failure::classified(message.into(), None, message.lines())
    }

    fn assert_an_exit(error: &ProviderError) {
        assert!(
            matches!(
                error,
                ProviderError::Exit {
                    status: ExitStatus::Code(1),
                    ..
                }
            ),
            "{error:?}"
        );
        assert!(!error.transient(), "{error:?}");
    }

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
        assert_eq!(failure.cause, Cause::Unclassified);
    }

    #[test]
    fn a_status_without_text_keeps_its_status() {
        let envelope = serde_json::json!({ "is_error": true, "api_error_status": 429 });
        let failure = Failure::reported(&envelope).unwrap();
        assert_eq!(failure.message, UNDESCRIBED);
        assert_eq!(failure.cause, Cause::Status(429));

        let exited = Failure::exited(envelope.to_string().as_bytes(), b"");
        let error = exited.into_error(CLI_NAME, ExitStatus::Code(1));
        assert!(error.transient(), "{error:?}");
    }

    #[test]
    fn a_status_without_text_is_told_in_the_diagnostics() {
        let envelope = serde_json::json!({ "is_error": true, "api_error_status": 529 });
        let failure = Failure::exited(envelope.to_string().as_bytes(), b"upstream overloaded\n");
        assert_eq!(failure.cause, Cause::Status(529));
        assert_eq!(failure.message, "upstream overloaded");

        let silent = Failure::exited(envelope.to_string().as_bytes(), b"  \n");
        assert_eq!(silent.message, UNDESCRIBED);
    }

    #[test]
    fn the_status_field_wins_over_the_text() {
        let failure = Failure::reported(&serde_json::json!({
            "is_error": true,
            "api_error_status": 503,
            "result": "API Error: 500 upstream"
        }))
        .unwrap();
        assert_eq!(failure.cause, Cause::Status(503));
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
        assert_eq!(failure.cause, Cause::Unclassified);
    }

    #[test]
    fn a_failure_without_a_status_is_an_exit_or_an_agent_error() {
        assert!(matches!(
            failure_from("no").into_error(CLI_NAME, ExitStatus::Code(2)),
            ProviderError::Exit {
                status: ExitStatus::Code(2),
                ..
            }
        ));
        assert!(matches!(
            failure_from("no").into_error(CLI_NAME, ExitStatus::Code(0)),
            ProviderError::Agent { .. }
        ));
    }

    #[test]
    fn a_connection_failure_is_a_transient_network_failure() {
        for message in [
            "API Error: Connection error.",
            "API Error: Connection error. (cause: connect ECONNREFUSED 127.0.0.1:443)",
            "API Error: request to https://api.anthropic.com failed, reason: socket hang up",
            "Retrying in 4 seconds\nAPI Error: read ECONNRESET",
        ] {
            for exit in [ExitStatus::Code(1), ExitStatus::Code(0)] {
                let error = failure_from(message).into_error(CLI_NAME, exit);

                assert!(
                    matches!(&error, ProviderError::Network { detail } if detail == message),
                    "{error:?}"
                );
                assert!(error.transient(), "{message}");
            }
        }
    }

    #[test]
    fn a_connection_failure_in_an_errors_list_is_read_from_its_own_entry() {
        let failure = Failure::reported(&serde_json::json!({
            "is_error": true,
            "errors": ["a hook said something", "API Error: Connection error."]
        }))
        .unwrap();

        assert!(
            failure
                .into_error(CLI_NAME, ExitStatus::Code(1))
                .transient()
        );
    }

    #[test]
    fn a_timeout_is_a_transient_network_failure() {
        for message in [
            "API Error: Request timed out.",
            "API Error: connect ETIMEDOUT 160.79.104.10:443",
        ] {
            let error = failure_from(message).into_error(CLI_NAME, ExitStatus::Code(1));

            assert!(matches!(&error, ProviderError::Network { .. }), "{error:?}");
            assert!(error.transient(), "{message}");
        }
    }

    #[test]
    fn an_overload_without_a_status_is_the_transient_overloaded_status() {
        for message in [
            "API Error: Overloaded",
            r#"API Error: {"type":"error","error":{"type":"overloaded_error"}}"#,
        ] {
            let error = failure_from(message).into_error(CLI_NAME, ExitStatus::Code(1));

            assert!(
                matches!(&error, ProviderError::Http { provider, source: Error::Api { status: OVERLOADED, message: text } } if provider == CLI_NAME && text == message),
                "{error:?}"
            );
            assert!(error.transient(), "{message}");
        }
    }

    #[test]
    fn a_rate_limit_body_without_a_status_is_a_transient_429() {
        let error = failure_from(
            r#"API Error: {"type":"error","error":{"type":"rate_limit_error","message":"slow down"}}"#,
        )
        .into_error(CLI_NAME, ExitStatus::Code(1));

        assert!(
            matches!(
                &error,
                ProviderError::Http {
                    source: Error::Api { status: 429, .. },
                    ..
                }
            ),
            "{error:?}"
        );
        assert!(error.transient());
    }

    #[test]
    fn a_status_wins_over_the_wording_after_it() {
        let error = failure_from("API Error: 401 Invalid bearer token: connection refused")
            .into_error(CLI_NAME, ExitStatus::Code(1));

        assert!(
            matches!(
                &error,
                ProviderError::Http {
                    source: Error::Api { status: 401, .. },
                    ..
                }
            ),
            "{error:?}"
        );
        assert!(!error.transient());
    }

    #[test]
    fn a_rejected_request_whose_message_names_a_transient_failure_is_not_retried() {
        for message in [
            "the model is overloaded right now",
            "connection refused by the upstream",
            "the tool call timed out",
            "overloaded; connection refused; timed out",
        ] {
            let envelope = serde_json::json!({
                "type": "result",
                "is_error": true,
                "result": format!(
                    r#"API Error: {{"type":"error","error":{{"type":"invalid_request_error","message":"{message}"}}}}"#
                )
            });

            let failure = Failure::exited(envelope.to_string().as_bytes(), b"");

            assert_an_exit(&failure.into_error(CLI_NAME, ExitStatus::Code(1)));
        }
    }

    #[test]
    fn a_rejected_status_on_stderr_is_a_rejection_that_is_not_recovered_from() {
        let failure = Failure::exited(b"", b"noise\nAPI Error: 400 bad");

        let error = failure.into_error(CLI_NAME, ExitStatus::Code(1));

        assert!(
            matches!(
                &error,
                ProviderError::Http {
                    source: Error::Api { status: 400, .. },
                    ..
                }
            ),
            "{error:?}"
        );
        assert!(!error.transient(), "{error:?}");
        assert!(!error.recoverable(), "{error:?}");
    }

    #[test]
    fn a_throttled_entry_in_an_errors_list_is_a_transient_429() {
        let failure = Failure::reported(&serde_json::json!({
            "is_error": true,
            "errors": ["x", "API Error: 429 {\"type\":\"error\",\"error\":{\"type\":\"rate_limit_error\"}}"]
        }))
        .unwrap();

        let error = failure.into_error(CLI_NAME, ExitStatus::Code(1));

        assert!(
            matches!(
                &error,
                ProviderError::Http {
                    source: Error::Api { status: 429, .. },
                    ..
                }
            ),
            "{error:?}"
        );
        assert!(error.transient(), "{error:?}");
        assert!(error.recoverable(), "{error:?}");
    }

    #[test]
    fn an_answer_on_a_failed_exit_is_not_read_as_the_failure() {
        let failure = Failure::exited(
            br#"{"type":"result","is_error":false,"result":"API Error: the request timed out"}"#,
            b"boom",
        );

        assert_eq!(failure.message, "boom");
        assert_an_exit(&failure.into_error(CLI_NAME, ExitStatus::Code(1)));
    }

    #[test]
    fn diagnostics_that_are_not_an_api_error_are_never_classified() {
        let failure = Failure::exited(
            b"",
            b"error: unknown option '--json-schema'\n\
              OTLPExporterError: Request failed: connect ECONNREFUSED 127.0.0.1:4318\n\
              read ECONNRESET; the request timed out; Overloaded",
        );

        assert_an_exit(&failure.into_error(CLI_NAME, ExitStatus::Code(1)));
    }

    #[test]
    fn a_hook_that_timed_out_is_not_a_network_failure() {
        let envelope = serde_json::json!({
            "type": "result",
            "subtype": "error_during_execution",
            "is_error": true,
            "errors": ["Hook PreToolUse:Bash timed out after 60s: connection refused"]
        });

        let reported = Failure::reported(&envelope).unwrap();
        let exited = Failure::exited(envelope.to_string().as_bytes(), b"");

        assert!(matches!(
            reported.into_error(CLI_NAME, ExitStatus::Code(0)),
            ProviderError::Agent { .. }
        ));
        assert_an_exit(&exited.into_error(CLI_NAME, ExitStatus::Code(1)));
    }

    #[test]
    fn any_other_failure_without_a_status_is_not_transient() {
        for message in [
            "Invalid API key. Please run /login",
            "API Error: Request was aborted.",
            "API Error: the untimed outcome was reconnection-free",
            "not signed in",
        ] {
            assert_an_exit(&failure_from(message).into_error(CLI_NAME, ExitStatus::Code(1)));
        }
    }
}
