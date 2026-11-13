use reqwest::header::CONTENT_RANGE;
use reqwest::header::HeaderMap;

/// The unit ranges of a log are counted in, matched without regard to case.
pub(super) const BYTES_UNIT: &str = "bytes";

/// What a `Content-Range` names a total it does not know by.
const UNKNOWN_TOTAL: &str = "*";

/// What a `Content-Range` that refuses a range starts its total with.
const UNSATISFIED: &str = "*/";

/// The span of a log a partial answer carries, as its `Content-Range` of
/// `bytes {first}-{last}/{total}` names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ContentRange {
    first: u64,
    last: u64,
    total: Option<u64>,
}

impl ContentRange {
    /// The span `headers` name, its total `None` when it is `*`; `None` for
    /// no `Content-Range`, one in another unit, or one that names no single
    /// span of bytes.
    pub(super) fn of(headers: &HeaderMap) -> Option<Self> {
        let (span, total) = in_bytes(headers)?.split_once('/')?;
        let (first, last) = span.split_once('-')?;
        let total = match total {
            UNKNOWN_TOTAL => None,
            total => Some(total.parse().ok()?),
        };
        Some(Self {
            first: first.parse().ok()?,
            last: last.parse().ok()?,
            total,
        })
    }

    /// The total a refusal's `Content-Range` of `bytes */{total}` names.
    pub(super) fn unsatisfied(headers: &HeaderMap) -> Option<u64> {
        in_bytes(headers)?.strip_prefix(UNSATISFIED)?.parse().ok()
    }

    /// Whether the log began before this span, when the span runs from
    /// `first` to `last` in order, ends where the log does, and is exactly
    /// the `arrived` bytes; `None` when it is not.
    pub(super) fn began_before(self, arrived: u64) -> Option<bool> {
        let length = self.last.checked_sub(self.first)?.checked_add(1)?;
        let ends = self
            .total
            .is_none_or(|total| self.last.checked_add(1) == Some(total));
        (length == arrived && ends).then_some(self.first > 0)
    }
}

/// What follows the unit of a `Content-Range` in `headers` that counts
/// bytes.
fn in_bytes(headers: &HeaderMap) -> Option<&str> {
    let (unit, rest) = headers.get(CONTENT_RANGE)?.to_str().ok()?.split_once(' ')?;
    unit.eq_ignore_ascii_case(BYTES_UNIT).then_some(rest)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_RANGE, value.parse().unwrap());
        headers
    }

    fn span(first: u64, last: u64, total: Option<u64>) -> ContentRange {
        ContentRange { first, last, total }
    }

    #[test]
    fn a_content_range_is_read_for_its_span_and_total_in_bytes_of_any_case() {
        for (value, expected) in [
            (
                "bytes 4000-4999/5000",
                Some(span(4_000, 4_999, Some(5_000))),
            ),
            ("BYTES 0-8/9", Some(span(0, 8, Some(9)))),
            ("Bytes 0-8/*", Some(span(0, 8, None))),
            ("bytes */5000", None),
            ("items 0-8/9", None),
            ("bytes 0-8/many", None),
            ("bytes x-8/9", None),
            ("bytes 0-y/9", None),
            ("bytes0-8/9", None),
        ] {
            assert_eq!(ContentRange::of(&headers(value)), expected, "{value}");
        }
        assert_eq!(ContentRange::of(&HeaderMap::new()), None);
    }

    #[test]
    fn a_refusal_names_the_total_it_could_not_serve() {
        for (value, expected) in [
            ("bytes */0", Some(0)),
            ("BYTES */15741", Some(15_741)),
            ("bytes 0-8/9", None),
            ("items */0", None),
            ("bytes */many", None),
        ] {
            assert_eq!(
                ContentRange::unsatisfied(&headers(value)),
                expected,
                "{value}"
            );
        }
        assert_eq!(ContentRange::unsatisfied(&HeaderMap::new()), None);
    }

    #[test]
    fn a_span_says_whether_the_log_began_before_it_only_when_it_ends_the_log_and_arrived_whole() {
        for (range, arrived, expected) in [
            (span(4_000, 4_999, Some(5_000)), 1_000, Some(true)),
            (span(0, 8, Some(9)), 9, Some(false)),
            (span(0, 8, None), 9, Some(false)),
            (span(1, 8, None), 8, Some(true)),
            (span(0, 8, Some(100)), 9, None),
            (span(4_000, 4_999, Some(5_000)), 999, None),
            (span(9, 8, Some(9)), 0, None),
            (span(0, u64::MAX, None), 0, None),
        ] {
            assert_eq!(
                range.began_before(arrived),
                expected,
                "{range:?} of {arrived}"
            );
        }
    }
}
