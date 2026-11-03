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
    const PATTERNS: &[&str] = &[
        "rate limit",
        "ratelimit",
        "usage limit reached",
        "5-hour limit reached",
        "weekly limit reached",
        "too many requests",
        "quota exceeded",
        "resource exhausted",
        "retry-after",
        "try again later",
    ];

    PATTERNS.iter().any(|needle| lower.contains(needle))
        || names_a_hit_limit(lower)
        || contains_standalone_429(lower)
}

/// Whether `lower` says the caller hit a limit, however the limit is named:
/// "you've hit your limit", "you've hit your weekly limit", "you've hit your
/// monthly spend limit".
///
/// The limit has to follow within a few words and in the same clause, so
/// "the retry hit your webhook twice" and "it hit your cache. limit the ttl"
/// are not read as one. A full stop inside a word, as in "gpt-4.1", ends no
/// clause. A context limit is the model's window, not a rate limit.
fn names_a_hit_limit(lower: &str) -> bool {
    const HIT: &str = "hit your ";
    const WORDS_BEFORE_THE_LIMIT: usize = 4;

    lower.match_indices(HIT).any(|(at, hit)| {
        let words: Vec<&str> = clause(&lower[at + hit.len()..])
            .split_whitespace()
            .take(WORDS_BEFORE_THE_LIMIT)
            .map(|word| word.trim_matches(|character: char| !character.is_alphanumeric()))
            .collect();
        words.iter().any(|word| is_limit_word(word)) && !words.contains(&"context")
    })
}

/// `text` up to the end of its first clause.
fn clause(text: &str) -> &str {
    const CLAUSE_ENDS: &[char] = &[',', ';', '|', '·', '∙', '\n'];

    let end = text
        .char_indices()
        .find(|&(at, character)| {
            CLAUSE_ENDS.contains(&character)
                || (character == '.'
                    && text[at + 1..]
                        .chars()
                        .next()
                        .is_none_or(char::is_whitespace))
        })
        .map_or(text.len(), |(at, _)| at);
    &text[..end]
}

fn is_limit_word(word: &str) -> bool {
    matches!(word, "limit" | "limits")
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
///
/// That includes the failures `abnegate-agent-cli` reports for an agent it
/// could not start, stop, read or finish in time.
pub fn is_hard_error(message: &str) -> bool {
    const PATTERNS: &[&str] = &[
        "failed to spawn",
        "failed to wait for",
        "failed to capture stdout",
        "failed to capture stderr",
        "process timed out",
        "timed out after",
        "could not be started",
        "could not be stopped",
        "agent output could not be parsed",
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
    fn rate_limit_error_usage_limit_with_reset_epoch() {
        assert!(is_rate_limit_error(
            "Claude AI usage limit reached|1790262000"
        ));
    }

    #[test]
    fn rate_limit_error_usage_limit_banner() {
        assert!(is_rate_limit_error(
            "Usage limit reached · continuing automatically at 3pm · esc to cancel"
        ));
    }

    #[test]
    fn rate_limit_error_five_hour_limit() {
        assert!(is_rate_limit_error("5-hour limit reached ∙ resets 3pm"));
    }

    #[test]
    fn rate_limit_error_weekly_limit() {
        assert!(is_rate_limit_error(
            "Opus weekly limit reached ∙ resets Oct 9, 10am"
        ));
    }

    #[test]
    fn rate_limit_error_named_limits() {
        assert!(is_rate_limit_error(
            "You've hit your weekly limit · resets Oct 9, 10am"
        ));
        assert!(is_rate_limit_error(
            "You've hit your session limit · resets 3pm"
        ));
        assert!(is_rate_limit_error("You've hit your Opus limit"));
        assert!(is_rate_limit_error("You've hit your monthly spend limit."));
        assert!(is_rate_limit_error("You've hit your limits for today"));
    }

    #[test]
    fn rate_limit_error_hit_your_without_a_limit() {
        assert!(!is_rate_limit_error("The retry hit your webhook twice"));
        assert!(!is_rate_limit_error(
            "It hit your cache. Limit the TTL instead"
        ));
    }

    #[test]
    fn rate_limit_error_limit_too_far_after_hit_your() {
        assert!(!is_rate_limit_error(
            "The probe hit your very own personal monthly limit dashboard"
        ));
        assert!(is_rate_limit_error(
            "You've hit your own personal monthly limit"
        ));
    }

    #[test]
    fn rate_limit_error_limit_across_a_clause_boundary() {
        for message in [
            "It hit your cache, limit the TTL instead",
            "It hit your cache; limit the TTL instead",
            "It hit your cache | limit 10",
            "It hit your cache · limit 10",
            "It hit your cache ∙ limit 10",
            "It hit your cache\nlimit 10",
        ] {
            assert!(!is_rate_limit_error(message), "{message}");
        }
    }

    #[test]
    fn rate_limit_error_a_dotted_model_name_stays_in_its_clause() {
        assert!(is_rate_limit_error("You've hit your GPT-4.1 limit"));
        assert!(is_rate_limit_error(
            "You've hit your claude-3.5 usage limits"
        ));
        assert!(!is_rate_limit_error("It hit your cache.limit the TTL"));
    }

    #[test]
    fn rate_limit_error_a_context_limit_is_not_a_rate_limit() {
        assert!(!is_rate_limit_error("You've hit your context limit"));
        assert!(!is_rate_limit_error("You've hit your context window limit"));
    }

    #[test]
    fn rate_limit_error_a_word_that_only_starts_with_limit() {
        assert!(!is_rate_limit_error(
            "The demo hit your limitless potential"
        ));
    }

    #[test]
    fn rate_limit_error_other_limits_reached_are_not_rate_limits() {
        assert!(!is_rate_limit_error("Context limit reached"));
        assert!(!is_rate_limit_error(
            "Subagent nesting limit reached (depth 3)"
        ));
        assert!(!is_rate_limit_error("Budget limit reached ($5.00 of $5)"));
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
    fn hard_error_agent_could_not_be_started() {
        assert!(is_hard_error(
            "claude: agent command claude could not be started: No such file or directory (os error 2)"
        ));
    }

    #[test]
    fn hard_error_agent_could_not_be_stopped() {
        assert!(is_hard_error("the agent could not be stopped"));
        assert!(is_hard_error(
            "claude: agent output could not be parsed: the agent could not be stopped"
        ));
    }

    #[test]
    fn hard_error_agent_output_could_not_be_parsed() {
        assert!(is_hard_error(
            "codex: agent output could not be parsed: expected value at line 1 column 1"
        ));
    }

    #[test]
    fn hard_error_agent_timed_out() {
        assert!(is_hard_error("claude: agent command timed out after 5s"));
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
