use serde_json::Value;

use crate::provider::error::TOO_MANY_REQUESTS;

const API_ERROR_PREFIX: &str = "API Error: ";
const ERROR_TYPE: &str = "/error/type";
const INTERNAL_SERVER_ERROR: u16 = 500;
const GATEWAY_TIMEOUT: u16 = 504;
pub(super) const OVERLOADED: u16 = 529;
const ERROR_TYPE_STATUSES: &[(&str, u16)] = &[
    ("overloaded_error", OVERLOADED),
    ("rate_limit_error", TOO_MANY_REQUESTS),
    ("api_error", INTERNAL_SERVER_ERROR),
    ("timeout_error", GATEWAY_TIMEOUT),
];
const NETWORK_FAILURES: &[&str] = &[
    "can't reach the api server",
    "check your internet connection",
    "connection closed",
    "connection dropped",
    "connection error",
    "connection lost",
    "connection refused",
    "connection reset",
    "connection to the api was lost",
    "econnrefused",
    "econnreset",
    "etimedout",
    "network error",
    "no internet route",
    "no response from api",
    "socket hang up",
    "timed out",
];
const OVERLOAD_FAILURES: &[&str] = &["overloaded"];

/// What the `API Error: ` lines of a CLI failure say went wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Cause {
    /// The API answered with this status.
    Status(u16),
    /// The API could not be reached, or did not answer in time.
    Network,
    /// Nothing the messages API route would retry.
    Unclassified,
}

impl Cause {
    /// The status the first `API Error: ` line among `lines` that has one
    /// reports, or else a network failure when one of them names one.
    pub(super) fn read<'a>(lines: impl IntoIterator<Item = &'a str>) -> Self {
        let mut cause = Self::Unclassified;
        for line in lines {
            match Self::reported_by(line) {
                status @ Self::Status(_) => return status,
                Self::Network => cause = Self::Network,
                Self::Unclassified => {}
            }
        }
        cause
    }

    /// What `line` reports, when it is an `API Error: ` line: a three-digit
    /// status, else the type of the API's JSON error body, which is final
    /// whatever the body's message says, else an overload or a network
    /// failure in so many words.
    fn reported_by(line: &str) -> Self {
        let Some(rest) = line.trim_start().strip_prefix(API_ERROR_PREFIX) else {
            return Self::Unclassified;
        };
        if let Some(status) = numbered(rest) {
            return Self::Status(status);
        }
        if let Some(kind) = error_type(rest) {
            return status_of(&kind).map_or(Self::Unclassified, Self::Status);
        }
        let lower = rest.to_lowercase();
        if names_any(&lower, OVERLOAD_FAILURES) {
            Self::Status(OVERLOADED)
        } else if names_any(&lower, NETWORK_FAILURES) {
            Self::Network
        } else {
            Self::Unclassified
        }
    }
}

fn numbered(rest: &str) -> Option<u16> {
    let digits = rest
        .split(|character: char| !character.is_ascii_digit())
        .next()?;
    if digits.len() != 3 {
        return None;
    }
    digits.parse().ok()
}

fn error_type(rest: &str) -> Option<String> {
    let body = serde_json::Deserializer::from_str(rest.trim())
        .into_iter::<Value>()
        .next()?
        .ok()?;
    body.pointer(ERROR_TYPE)?.as_str().map(str::to_string)
}

fn status_of(kind: &str) -> Option<u16> {
    ERROR_TYPE_STATUSES
        .iter()
        .find(|(name, _)| *name == kind)
        .map(|(_, status)| *status)
}

/// Whether `lower` holds one of `phrases` as words of its own, so
/// `timed out` is not read inside `untimed outcome`.
fn names_any(lower: &str, phrases: &[&str]) -> bool {
    phrases.iter().any(|phrase| {
        lower.match_indices(phrase).any(|(at, _)| {
            let after = at + phrase.len();
            !lower[..at].ends_with(is_word_character)
                && !lower[after..].starts_with(is_word_character)
        })
    })
}

fn is_word_character(character: char) -> bool {
    character.is_alphanumeric() || character == '_'
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLI_2_1_289_NETWORK_FAILURES: &[&str] = &[
        "API Error: Connection error.",
        "API Error: Unable to connect to API. Check your internet connection",
        "API Error: Connection dropped (EPIPE)",
        "API Error: Connection dropped (ECONNABORTED)",
        "API Error: Connection refused \u{2014} a firewall or proxy may be blocking it (ConnectionRefused)",
        "API Error: Can't reach the API server \u{2014} check your internet or DNS (EAI_AGAIN)",
        "API Error: No internet route \u{2014} check your connection or VPN (ENETUNREACH)",
        "API Error: Connection to the API was lost (UND_ERR_SOCKET). This is usually temporary \u{2014} try again.",
        "API Error: Connection lost mid-response. The response above may be incomplete.",
        "API Error: Connection lost before a response was produced. Try again.",
        "API Error: Connection lost while your computer was asleep",
        "API Error: Connection closed before the response finished",
        "API Error: Request timed out.",
        "API Error: Request timed out. Check your internet connection and proxy settings",
        "API Error: No response from API (waited 30s). If a proxy or gateway on your network holds responses until they complete, raise API_TIMEOUT_MS or CLAUDE_STREAM_FIRST_BYTE_TIMEOUT_MS to wait longer.",
    ];

    fn cause_of(line: &str) -> Cause {
        Cause::read([line])
    }

    #[test]
    fn a_status_is_only_read_from_an_api_error() {
        assert_eq!(
            cause_of("API Error: 429 {\"type\":\"error\"}"),
            Cause::Status(TOO_MANY_REQUESTS)
        );
        assert_eq!(
            cause_of("API Error: 401 Invalid API key"),
            Cause::Status(401)
        );
        assert_eq!(cause_of("  API Error: 503 upstream"), Cause::Status(503));
        assert_eq!(
            cause_of("API Error: Request was aborted."),
            Cause::Unclassified
        );
        assert_eq!(cause_of("API Error: 4290 nope"), Cause::Unclassified);
        assert_eq!(cause_of("Error: 429"), Cause::Unclassified);
        assert_eq!(
            cause_of("the model said API Error: 429"),
            Cause::Unclassified
        );
    }

    #[test]
    fn the_type_of_an_api_error_body_names_its_status() {
        for (kind, status) in ERROR_TYPE_STATUSES {
            let line = format!(
                r#"API Error: {{"type":"error","error":{{"type":"{kind}","message":"x"}}}} · retried"#
            );
            assert_eq!(cause_of(&line), Cause::Status(*status), "{kind}");
        }
        assert_eq!(
            cause_of(r#"API Error: {"type":"error","error":{"type":"invalid_request_error"}}"#),
            Cause::Unclassified
        );
        assert_eq!(cause_of("API Error: {not json"), Cause::Unclassified);
    }

    #[test]
    fn the_type_of_an_api_error_body_is_final_whatever_its_message_says() {
        for message in [
            "the model is overloaded",
            "connection refused by the upstream",
            "the tool call timed out",
        ] {
            let line = format!(
                r#"API Error: {{"type":"error","error":{{"type":"invalid_request_error","message":"{message}"}}}}"#
            );
            assert_eq!(cause_of(&line), Cause::Unclassified, "{message}");
        }
        assert_eq!(
            cause_of(
                r#"API Error: {"type":"error","error":{"type":"rate_limit_error","message":"connection refused"}}"#
            ),
            Cause::Status(TOO_MANY_REQUESTS)
        );
    }

    #[test]
    fn a_json_body_without_an_error_type_is_read_for_its_wording() {
        assert_eq!(
            cause_of(r#"API Error: {"message":"Overloaded"}"#),
            Cause::Status(OVERLOADED)
        );
    }

    #[test]
    fn the_wording_of_the_cli_for_a_lost_connection_or_a_timeout_is_a_network_failure() {
        for line in CLI_2_1_289_NETWORK_FAILURES {
            assert_eq!(cause_of(line), Cause::Network, "{line}");
        }
    }

    #[test]
    fn the_wording_of_the_cli_for_an_overload_is_the_overloaded_status() {
        assert_eq!(
            cause_of(
                "API Error: Repeated 529 Overloaded errors. The API is at capacity \u{2014} this is usually temporary. Try again in a moment."
            ),
            Cause::Status(OVERLOADED)
        );
    }

    #[test]
    fn a_certificate_the_cli_cannot_trust_is_not_a_network_failure() {
        assert_eq!(
            cause_of("API Error: Unable to connect to API: SSL certificate has expired"),
            Cause::Unclassified
        );
    }

    #[test]
    fn the_first_status_wins_over_a_network_failure_on_any_line() {
        assert_eq!(
            Cause::read(["API Error: Connection error.", "API Error: 503 upstream"]),
            Cause::Status(503)
        );
        assert_eq!(
            Cause::read(["noise", "API Error: Connection error."]),
            Cause::Network
        );
    }

    #[test]
    fn a_failure_word_is_only_read_as_a_word_of_its_own() {
        assert!(names_any("request timed out.", NETWORK_FAILURES));
        assert!(names_any("connect etimedout 1.2.3.4", NETWORK_FAILURES));
        assert!(!names_any("untimed outcome", NETWORK_FAILURES));
        assert!(!names_any("xetimedout", NETWORK_FAILURES));
        assert!(!names_any("econnrefused_count", NETWORK_FAILURES));
    }
}
