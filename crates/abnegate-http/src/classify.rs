const RATE_LIMIT_EVENT: &str = "rate_limit_event";
const ALLOWED_STATUSES: [&str; 2] = ["\"status\":\"allowed\"", "\"status\":\"allowed_warning\""];

/// Whether `message` describes a rate limit, however the far side spelled it.
///
/// Agent and provider streams carry informational `rate_limit_event` records
/// whose status says the request was allowed; those are not rejections and
/// must not be read as one. A stream carries one record per line, so an
/// allowed record excuses only its own line, never a rejection elsewhere in
/// the same message.
pub fn is_rate_limit_error(message: &str) -> bool {
    records(message).any(|record| is_rate_limit_error_lower(&record.to_lowercase()))
}

/// The lines of `message` that could describe a failure.
fn records(message: &str) -> impl Iterator<Item = &str> {
    message
        .lines()
        .filter(|record| !is_allowed_rate_limit_event(record))
}

fn is_allowed_rate_limit_event(record: &str) -> bool {
    record.contains(RATE_LIMIT_EVENT)
        && ALLOWED_STATUSES
            .iter()
            .any(|status| record.contains(status))
}

fn is_rate_limit_error_lower(lower: &str) -> bool {
    const PATTERNS: [&str; 8] = [
        "rate limit",
        "ratelimit",
        "hit your limit",
        "too many requests",
        "quota exceeded",
        "resource exhausted",
        "retry-after",
        "try again later",
    ];

    PATTERNS.iter().any(|needle| lower.contains(needle)) || contains_standalone_429(lower)
}

/// Whether `429` appears as a status code rather than as part of something
/// else.
///
/// A rejection is inferred from text, and that text is often a whole JSON
/// stream: `4299` inside a UUID and `"input_tokens":429` both carry the digits
/// without being a status, so a bare substring search reports rate limits that
/// never happened.
fn contains_standalone_429(text: &str) -> bool {
    const CODE: &str = "429";

    let bytes = text.as_bytes();
    let mut start = 0;
    while start + CODE.len() <= bytes.len() {
        let Some(position) = text[start..].find(CODE) else {
            break;
        };
        let at = start + position;
        let after = at + CODE.len();

        let bounded = (at == 0 || !bytes[at - 1].is_ascii_alphanumeric())
            && (after >= bytes.len() || !bytes[after].is_ascii_alphanumeric());
        if bounded && !is_json_value(bytes, at) {
            return true;
        }
        start = at + 1;
    }
    false
}

/// Whether the number at `at` is a JSON object's value, as in `"tokens":429`.
///
/// The quote closing the key is what separates it from plain text like
/// `status:429`.
fn is_json_value(bytes: &[u8], at: usize) -> bool {
    let colon = if at > 0 && bytes[at - 1] == b':' {
        at - 1
    } else if at > 1 && bytes[at - 1] == b' ' && bytes[at - 2] == b':' {
        at - 2
    } else {
        return false;
    };

    colon > 0 && bytes[colon - 1] == b'"'
}

/// Whether `message` describes a failure that will not improve on its own and
/// should be escalated rather than retried quietly.
pub fn is_hard_error(message: &str) -> bool {
    const PATTERNS: [&str; 11] = [
        "failed to spawn",
        "failed to wait for",
        "failed to capture stdout",
        "failed to capture stderr",
        "process timed out",
        "timed out after",
        "connection reset",
        "service unavailable",
        "internal server error",
        "network error",
        "broken pipe",
    ];

    records(message).any(|record| {
        let lower = record.to_lowercase();
        is_rate_limit_error_lower(&lower) || PATTERNS.iter().any(|needle| lower.contains(needle))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_limit_error_rate_limit() {
        assert!(is_rate_limit_error("Error: rate limit exceeded"));
    }

    #[test]
    fn rate_limit_error_429() {
        assert!(is_rate_limit_error("HTTP 429 Too Many Requests"));
    }

    #[test]
    fn rate_limit_error_quota_exceeded() {
        assert!(is_rate_limit_error("API quota exceeded for project"));
    }

    #[test]
    fn rate_limit_error_resource_exhausted() {
        assert!(is_rate_limit_error("Resource exhausted: try again later"));
    }

    #[test]
    fn rate_limit_error_retry_after() {
        assert!(is_rate_limit_error("retry-after: 30"));
    }

    #[test]
    fn rate_limit_error_ratelimit_one_word() {
        assert!(is_rate_limit_error("ratelimit hit"));
    }

    #[test]
    fn rate_limit_error_too_many_requests() {
        assert!(is_rate_limit_error("too many requests"));
    }

    #[test]
    fn rate_limit_error_try_again_later() {
        assert!(is_rate_limit_error("Please try again later"));
    }

    #[test]
    fn rate_limit_error_limit_banner() {
        assert!(is_rate_limit_error(
            "You've hit your limit · resets 6am (UTC)"
        ));
    }

    #[test]
    fn rate_limit_error_case_insensitive() {
        assert!(is_rate_limit_error("RATE LIMIT EXCEEDED"));
        assert!(is_rate_limit_error("Rate Limit"));
    }

    #[test]
    fn rate_limit_error_negative() {
        assert!(!is_rate_limit_error("connection refused"));
        assert!(!is_rate_limit_error("file not found"));
        assert!(!is_rate_limit_error("success"));
        assert!(!is_rate_limit_error(""));
    }

    #[test]
    fn hard_error_spawn_failure() {
        assert!(is_hard_error("Failed to spawn process"));
    }

    #[test]
    fn hard_error_wait_failure() {
        assert!(is_hard_error("Failed to wait for child process"));
    }

    #[test]
    fn hard_error_stdout_capture() {
        assert!(is_hard_error("Failed to capture stdout"));
    }

    #[test]
    fn hard_error_stderr_capture() {
        assert!(is_hard_error("Failed to capture stderr"));
    }

    #[test]
    fn hard_error_timeout() {
        assert!(is_hard_error("Process timed out"));
        assert!(is_hard_error("Timed out after 3600 seconds"));
    }

    #[test]
    fn hard_error_network_errors() {
        assert!(is_hard_error("Connection reset by peer"));
        assert!(is_hard_error("Service unavailable"));
        assert!(is_hard_error("Internal server error"));
        assert!(is_hard_error("Network error: DNS resolution failed"));
        assert!(is_hard_error("Broken pipe"));
    }

    #[test]
    fn hard_error_includes_rate_limits() {
        assert!(is_hard_error("rate limit exceeded"));
        assert!(is_hard_error("429"));
    }

    #[test]
    fn hard_error_case_insensitive() {
        assert!(is_hard_error("FAILED TO SPAWN"));
        assert!(is_hard_error("Service Unavailable"));
    }

    #[test]
    fn hard_error_negative() {
        assert!(!is_hard_error("syntax error in code"));
        assert!(!is_hard_error("test failed"));
        assert!(!is_hard_error("compilation error"));
        assert!(!is_hard_error(""));
    }

    #[test]
    fn rate_limit_error_partial_match_boundary() {
        assert!(!is_rate_limit_error("highly rated code"));
        assert!(is_rate_limit_error("some rate limit error occurred"));
    }

    #[test]
    fn hard_error_combined_messages() {
        assert!(is_hard_error(
            "Failed to spawn /usr/bin/agent: No such file or directory"
        ));
        assert!(is_hard_error("Connection reset by peer while streaming"));
    }

    #[test]
    fn rate_limit_error_unicode_safe() {
        assert!(!is_rate_limit_error("错误：无法连接"));
        assert!(is_rate_limit_error(
            "Error: rate limit exceeded. 请稍后再试"
        ));
    }

    #[test]
    fn hard_error_long_message() {
        let message = "a".repeat(100_000) + " failed to spawn process";
        assert!(is_hard_error(&message));
    }

    #[test]
    fn standalone_429() {
        assert!(is_rate_limit_error("429"));
        assert!(is_rate_limit_error("HTTP 429 Too Many Requests"));
        assert!(is_rate_limit_error("error 429"));
        assert!(is_rate_limit_error("status:429"));
        assert!(is_rate_limit_error("(429)"));
    }

    #[test]
    fn a_429_inside_a_uuid_is_not_a_rate_limit() {
        assert!(!is_rate_limit_error(
            r#"{"uuid":"26355e0a-810a-4299-a2e5-4cea5f762d2e"}"#
        ));
        assert!(!is_rate_limit_error("a429b"));
        assert!(!is_rate_limit_error("x4290y"));
    }

    #[test]
    fn a_429_inside_a_streamed_record_is_not_a_rate_limit() {
        let record = r#"{"type":"user","message":{"role":"user","content":[{"tool_use_id":"toolu_xyz","type":"tool_result","content":"ok"}]},"session_id":"1a814bc2-22af-42a1-906a-9c3e03e9dd8c","uuid":"26355e0a-810a-4299-a2e5-4cea5f762d2e","tool_use_result":"ok"}"#;
        assert!(!is_rate_limit_error(record));
    }

    #[test]
    fn a_429_token_count_is_not_a_rate_limit() {
        assert!(!is_rate_limit_error(
            r#"{"input_tokens":429,"output_tokens":100}"#
        ));
        assert!(!is_rate_limit_error(
            r#"{"cache_creation_input_tokens":429}"#
        ));
        assert!(!is_rate_limit_error(r#"{"tokens": 429}"#));
        assert!(is_rate_limit_error("status:429"));
        assert!(is_rate_limit_error(
            r#"{"error":"HTTP 429 Too Many Requests"}"#
        ));
    }

    #[test]
    fn an_allowed_warning_rate_limit_event_is_not_an_error() {
        let record = r#"{"type":"rate_limit_event","rate_limit_info":{"status":"allowed_warning","resetsAt":1772096400,"rateLimitType":"seven_day","utilization":0.81,"isUsingOverage":false,"surpassedThreshold":0.75},"uuid":"addd358a-c64f-48cd-b9a2-97af382e0fd6","session_id":"f450b721-71e2-4d8f-8a2d-07827df35f1d"}"#;
        assert!(!is_rate_limit_error(record));
    }

    #[test]
    fn an_allowed_rate_limit_event_is_not_an_error() {
        let record = r#"{"type":"rate_limit_event","rate_limit_info":{"status":"allowed","utilization":0.5}}"#;
        assert!(!is_rate_limit_error(record));
    }

    #[test]
    fn a_bare_allowed_rate_limit_event_is_not_an_error() {
        assert!(!is_rate_limit_error(
            r#"rate_limit_event "status":"allowed""#
        ));
        assert!(!is_rate_limit_error(
            r#"rate_limit_event "status":"allowed_warning""#
        ));
    }

    #[test]
    fn an_allowed_event_does_not_hide_a_rejection_on_another_line() {
        let stream = concat!(
            r#"{"type":"rate_limit_event","rate_limit_info":{"status":"allowed_warning","rateLimitType":"seven_day"}}"#,
            "\n",
            r#"{"type":"result","is_error":true,"result":"API Error: 429 Too Many Requests"}"#,
        );

        assert!(is_rate_limit_error(stream));
        assert!(is_hard_error(stream));
    }

    #[test]
    fn a_rejected_rate_limit_event_is_an_error() {
        let stream = concat!(
            r#"{"type":"rate_limit_event","rate_limit_info":{"status":"allowed"}}"#,
            "\r\n",
            r#"{"type":"rate_limit_event","rate_limit_info":{"status":"rejected","rateLimitType":"five_hour"}}"#,
        );

        assert!(is_rate_limit_error(stream));
    }

    #[test]
    fn an_allowed_rate_limit_event_is_not_a_hard_error() {
        let record = r#"{"type":"rate_limit_event","rate_limit_info":{"status":"allowed_warning","rateLimitType":"seven_day","utilization":0.81}}"#;

        assert!(!is_hard_error(record));
    }
}
