use std::time::Duration;
use std::time::SystemTime;

const LABELS: &[&str] = &[
    "retry-after",
    "retry_after",
    "retry after",
    "retry in",
    "try again in",
];

const SEPARATORS: &[char] = &[':', '=', ' ', '"', '\t'];

const MILLISECOND_UNITS: &[&str] = &["ms", "millis"];

const MILLISECONDS_PER_SECOND: f64 = 1_000.0;

/// Reads how long a server asked a caller to wait before trying again.
///
/// [`RetryAfter::from_header`] reads a `Retry-After` header and
/// [`RetryAfter::from_text`] reads the same hint out of a failure message.
/// Neither caps the delay: a server can ask for a day, and each caller decides
/// how long it is willing to wait.
///
/// ```
/// use abnegate_http::RetryAfter;
/// use std::time::{Duration, SystemTime};
///
/// assert_eq!(
///     RetryAfter::from_header("120", SystemTime::now()),
///     Some(Duration::from_secs(120))
/// );
/// assert_eq!(
///     RetryAfter::from_text("429 slow down, try again in 500ms"),
///     Some(Duration::from_millis(500))
/// );
/// ```
#[derive(Debug)]
#[non_exhaustive]
pub struct RetryAfter;

impl RetryAfter {
    /// The delay a `Retry-After` header value asks for, measured from `now`.
    ///
    /// The value is either delta-seconds or an HTTP-date (RFC 9110 10.2.3).
    /// Delta-seconds may carry one leading `+`. Delta-seconds of `0` is
    /// `Some(Duration::ZERO)`, and delta-seconds too large for a `u64`, with or
    /// without its `+`, saturates to [`Duration::MAX`] (RFC 9111 1.2.2), so
    /// capping the wait is left to the caller.
    ///
    /// Only RFC 9110's three date forms are read: IMF-fixdate, which must end
    /// in `GMT`, RFC 850 and asctime, with day and month names capitalised as
    /// those forms spell them. Other RFC 5322 dates, such as a numeric UTC
    /// offset (`+0000`), lowercase names or a missing weekday, are `None`, and
    /// so is a date before 1970. An RFC 850 date's two-digit year is read as a
    /// year from 1970 to 2069. A date already past asks for no wait at all, and
    /// the delay to a date keeps the fraction of a second `now` carries, so a
    /// caller never retries before the instant the date names. A value that is
    /// neither delta-seconds nor such a date, such as `soon`, `-1` or `1.5`, is
    /// `None`.
    ///
    /// ```
    /// use abnegate_http::RetryAfter;
    /// use std::time::{Duration, UNIX_EPOCH};
    ///
    /// let now = UNIX_EPOCH + Duration::from_secs(784_111_740);
    ///
    /// assert_eq!(
    ///     RetryAfter::from_header("Sun, 06 Nov 1994 08:49:37 GMT", now),
    ///     Some(Duration::from_secs(37))
    /// );
    /// assert_eq!(
    ///     RetryAfter::from_header("Sun, 06 Nov 1994 08:48:00 GMT", now),
    ///     Some(Duration::ZERO)
    /// );
    /// assert_eq!(RetryAfter::from_header("soon", now), None);
    /// ```
    pub fn from_header(value: &str, now: SystemTime) -> Option<Duration> {
        let value = value.trim();
        if let Ok(seconds) = value.parse::<u64>() {
            return Some(Duration::from_secs(seconds));
        }
        if is_delta_seconds(value) {
            return Some(Duration::MAX);
        }
        let at = httpdate::parse_http_date(value).ok()?;
        Some(at.duration_since(now).unwrap_or(Duration::ZERO))
    }

    /// The delay a free-text failure message asks for, such as
    /// `retry-after: 42`, `please retry after 42 seconds` or
    /// `try again in 500ms`.
    ///
    /// The text is matched case-insensitively. A number after a retry label may
    /// be fractional and is read as milliseconds when `ms` or `millis` follows
    /// it, and as seconds after any other unit or none, so `5 minutes` reads as
    /// five seconds.
    ///
    /// The labels are tried in the order `retry-after`, `retry_after`,
    /// `retry after`, `retry in` and `try again in`, and only the first
    /// occurrence of each label is read: when the text right after it is not a
    /// number above zero, that label yields nothing, even if it appears again
    /// later, and the next label is tried. Text with no such hint is `None`.
    ///
    /// A full stop ending the sentence is not part of the number, so
    /// `retry in 1.5.` reads as one and a half seconds.
    ///
    /// A number with no nonzero digit, such as `0` or `0.0`, is refused, unlike
    /// [`RetryAfter::from_header`]'s `0`, but a positive number too small for a
    /// nanosecond, such as `0.0000000001`, rounds to `Some(Duration::ZERO)`. A
    /// number too large for a [`Duration`] saturates to [`Duration::MAX`].
    ///
    /// ```
    /// use abnegate_http::RetryAfter;
    /// use std::time::Duration;
    ///
    /// assert_eq!(
    ///     RetryAfter::from_text("Rate limited, please retry after 42 seconds"),
    ///     Some(Duration::from_secs(42))
    /// );
    /// assert_eq!(
    ///     RetryAfter::from_text("try again in 1.5s"),
    ///     Some(Duration::from_millis(1_500))
    /// );
    /// assert_eq!(RetryAfter::from_text("retry-after: soon"), None);
    /// ```
    pub fn from_text(text: &str) -> Option<Duration> {
        let lowered = text.to_ascii_lowercase();
        LABELS
            .iter()
            .find_map(|label| lowered.split(label).nth(1).and_then(leading_duration))
    }
}

fn is_delta_seconds(value: &str) -> bool {
    let digits = value.strip_prefix('+').unwrap_or(value);
    !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit())
}

fn has_nonzero_digit(number: &str) -> bool {
    number.bytes().any(|byte| matches!(byte, b'1'..=b'9'))
}

fn leading_duration(tail: &str) -> Option<Duration> {
    let tail = tail.trim_start_matches(SEPARATORS);
    let end = tail
        .find(|character: char| !character.is_ascii_digit() && character != '.')
        .unwrap_or(tail.len());
    let number = tail[..end].trim_end_matches('.');
    if !has_nonzero_digit(number) {
        return None;
    }
    let value: f64 = number.parse().ok()?;
    let unit = tail[end..].trim_start();
    let seconds = if MILLISECOND_UNITS
        .iter()
        .any(|millisecond| unit.starts_with(millisecond))
    {
        value / MILLISECONDS_PER_SECOND
    } else {
        value
    };
    Some(Duration::try_from_secs_f64(seconds).unwrap_or(Duration::MAX))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;
    use std::time::SystemTime;
    use std::time::UNIX_EPOCH;

    use super::RetryAfter;

    fn now() -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(784_111_740)
    }

    #[test]
    fn header_reads_delta_seconds_and_http_dates() {
        let now = now();
        assert_eq!(
            RetryAfter::from_header("3", now),
            Some(Duration::from_secs(3))
        );
        assert_eq!(
            RetryAfter::from_header(" 30 ", now),
            Some(Duration::from_secs(30))
        );
        assert_eq!(
            RetryAfter::from_header("Sun, 06 Nov 1994 08:49:37 GMT", now),
            Some(Duration::from_secs(37))
        );
        assert_eq!(
            RetryAfter::from_header("Sun, 06 Nov 1994 08:48:00 GMT", now),
            Some(Duration::ZERO),
            "a date already past is retried at once"
        );
        assert_eq!(RetryAfter::from_header("soon", now), None);
        assert_eq!(RetryAfter::from_header("", now), None);
    }

    #[test]
    fn header_delay_is_not_capped() {
        let now = now();
        assert_eq!(
            RetryAfter::from_header("61", now),
            Some(Duration::from_secs(61)),
            "a long delta is the caller's to cap"
        );
        assert_eq!(
            RetryAfter::from_header("Sun, 06 Nov 1994 09:49:37 GMT", now),
            Some(Duration::from_secs(3_637)),
            "a distant date is the caller's to cap"
        );
    }

    #[test]
    fn header_date_keeps_the_fraction_of_a_second_now_carries() {
        let now = now() + Duration::from_millis(250);
        assert_eq!(
            RetryAfter::from_header("Sun, 06 Nov 1994 08:49:37 GMT", now),
            Some(Duration::from_millis(36_750)),
            "rounding to whole seconds would retry before the named instant"
        );
    }

    #[test]
    fn header_reads_rfc_850_and_asctime_dates() {
        let now = now();
        assert_eq!(
            RetryAfter::from_header("Sunday, 06-Nov-94 08:49:37 GMT", now),
            Some(Duration::from_secs(37))
        );
        assert_eq!(
            RetryAfter::from_header("Sun Nov  6 08:49:37 1994", now),
            Some(Duration::from_secs(37))
        );
    }

    #[test]
    fn header_refuses_rfc_5322_dates_outside_rfc_9110() {
        let now = now();
        for value in [
            "Sun, 06 Nov 1994 08:49:37 +0000",
            "sun, 06 nov 1994 08:49:37 gmt",
            "06 Nov 1994 08:49:37 GMT",
        ] {
            assert_eq!(
                RetryAfter::from_header(value, now),
                None,
                "{value} is not one of RFC 9110's three date forms"
            );
        }
    }

    #[test]
    fn header_saturates_overflowing_delta_seconds() {
        assert_eq!(
            RetryAfter::from_header("18446744073709551616", now()),
            Some(Duration::MAX),
            "RFC 9111 saturates a delta-seconds too large to hold"
        );
        assert_eq!(
            RetryAfter::from_header("+18446744073709551616", now()),
            Some(Duration::MAX),
            "a leading plus saturates the same way"
        );
    }

    #[test]
    fn header_accepts_zero_and_a_leading_plus() {
        let now = now();
        assert_eq!(RetryAfter::from_header("0", now), Some(Duration::ZERO));
        assert_eq!(
            RetryAfter::from_header("+5", now),
            Some(Duration::from_secs(5)),
            "a leading plus in delta-seconds stays accepted"
        );
        assert_eq!(RetryAfter::from_header("+", now), None);
        assert_eq!(RetryAfter::from_header("++5", now), None);
    }

    #[test]
    fn header_refuses_dates_before_1970() {
        assert_eq!(
            RetryAfter::from_header("Wed, 31 Dec 1969 23:59:59 GMT", now()),
            None
        );
    }

    #[test]
    fn header_reads_an_rfc_850_two_digit_year_from_1970_to_2069() {
        assert_eq!(
            RetryAfter::from_header("Thursday, 01-Jan-70 00:00:37 GMT", UNIX_EPOCH),
            Some(Duration::from_secs(37)),
            "70 is 1970"
        );
        let now = now();
        let two_digit = RetryAfter::from_header("Wednesday, 06-Nov-69 08:49:37 GMT", now);
        assert!(two_digit.is_some());
        assert_eq!(
            two_digit,
            RetryAfter::from_header("Wed, 06 Nov 2069 08:49:37 GMT", now),
            "69 is 2069"
        );
    }

    #[test]
    fn header_refuses_negative_and_fractional_deltas() {
        let now = now();
        assert_eq!(RetryAfter::from_header("-1", now), None);
        assert_eq!(RetryAfter::from_header("1.5", now), None);
    }

    #[test]
    fn text_hints_are_read_verbatim() {
        for message in [
            "429 rate limit exceeded; retry-after: 42",
            "Rate limited, please retry after 42 seconds",
            "429 slow down, try again in 42s",
        ] {
            assert_eq!(
                RetryAfter::from_text(message),
                Some(Duration::from_secs(42)),
                "{message} carries a hint"
            );
        }
        assert_eq!(
            RetryAfter::from_text("rate limit hit, retry-after: 500ms"),
            Some(Duration::from_millis(500))
        );
        assert_eq!(
            RetryAfter::from_text("try again in 1.5s"),
            Some(Duration::from_millis(1_500))
        );
        assert_eq!(
            RetryAfter::from_text("retry_after: 200ms"),
            Some(Duration::from_millis(200))
        );
        assert_eq!(
            RetryAfter::from_text("retry in 250 millis"),
            Some(Duration::from_millis(250))
        );
        assert_eq!(
            RetryAfter::from_text("Retry-After=\"7\""),
            Some(Duration::from_secs(7))
        );
        assert_eq!(
            RetryAfter::from_text("RATE LIMITED, TRY AGAIN IN 9S"),
            Some(Duration::from_secs(9)),
            "labels match regardless of case"
        );
        assert_eq!(
            RetryAfter::from_text("Retry-After:\t5"),
            Some(Duration::from_secs(5)),
            "a tab separates a label from its number"
        );
        assert_eq!(
            RetryAfter::from_text("重试 retry-after: 5 秒"),
            Some(Duration::from_secs(5)),
            "non-ASCII text around the hint is skipped"
        );
    }

    #[test]
    fn text_reads_any_unit_but_milliseconds_as_seconds() {
        assert_eq!(
            RetryAfter::from_text("retry in 5 minutes"),
            Some(Duration::from_secs(5))
        );
    }

    #[test]
    fn text_rounds_a_tiny_positive_hint_to_zero() {
        assert_eq!(
            RetryAfter::from_text("retry in 0.0000000001"),
            Some(Duration::ZERO)
        );
        let message = format!("retry in 0.{}1", "0".repeat(400));
        assert_eq!(
            RetryAfter::from_text(&message),
            Some(Duration::ZERO),
            "a positive hint that underflows to zero still asks for a wait"
        );
    }

    #[test]
    fn text_hint_ignores_a_full_stop_ending_the_sentence() {
        assert_eq!(
            RetryAfter::from_text("retry in 1.5."),
            Some(Duration::from_millis(1_500))
        );
        assert_eq!(
            RetryAfter::from_text("retry in 5."),
            Some(Duration::from_secs(5))
        );
        assert_eq!(
            RetryAfter::from_text("retry-after: 42."),
            Some(Duration::from_secs(42))
        );
        assert_eq!(RetryAfter::from_text("retry in ."), None);
    }

    #[test]
    fn text_saturates_a_hint_too_large_for_a_duration() {
        assert_eq!(
            RetryAfter::from_text("retry-after: 18446744073709551616"),
            Some(Duration::MAX)
        );
        let message = format!("retry-after: {}", "9".repeat(400));
        assert_eq!(
            RetryAfter::from_text(&message),
            Some(Duration::MAX),
            "a number that parses to infinity saturates too"
        );
    }

    #[test]
    fn text_delay_is_not_capped() {
        assert_eq!(
            RetryAfter::from_text("rate limit hit, retry-after: 86400"),
            Some(Duration::from_secs(86_400)),
            "a day-long hint is the caller's to cap"
        );
    }

    #[test]
    fn text_without_a_positive_hint_is_none() {
        for message in [
            "retry in 0",
            "retry in 0.0",
            "retry in 00",
            "retry-after: -3",
            "retry-after: soon",
            "429 Too Many Requests",
            "",
        ] {
            assert_eq!(RetryAfter::from_text(message), None, "{message}");
        }
    }

    #[test]
    fn text_labels_are_tried_in_order() {
        assert_eq!(
            RetryAfter::from_text("try again in 5s; retry-after: 3"),
            Some(Duration::from_secs(3)),
            "retry-after outranks try again in wherever it appears"
        );
        assert_eq!(
            RetryAfter::from_text("retry-after: soon, try again in 4s"),
            Some(Duration::from_secs(4)),
            "a label whose tail does not parse falls through to the next"
        );
    }

    #[test]
    fn text_reads_only_the_first_occurrence_of_each_label() {
        assert_eq!(
            RetryAfter::from_text("retry interval 3s, retry in 2s"),
            None,
            "retry in first matches inside retry interval"
        );
        assert_eq!(
            RetryAfter::from_text("retry-after header missing; retry-after: 5"),
            None,
            "a later retry-after is not read"
        );
    }
}
