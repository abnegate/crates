use chrono::DateTime;
use std::time::Duration;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

/// The instant an RFC 3339 timestamp names, or `None` when it names none or
/// one this platform's clock cannot hold.
pub(super) fn instant(timestamp: &str) -> Option<SystemTime> {
    let parsed = DateTime::parse_from_rfc3339(timestamp).ok()?;
    let seconds = parsed.timestamp();
    let fraction = Duration::from_nanos(u64::from(parsed.timestamp_subsec_nanos()));
    let whole = Duration::from_secs(seconds.unsigned_abs());
    let second = match seconds < 0 {
        true => UNIX_EPOCH.checked_sub(whole),
        false => UNIX_EPOCH.checked_add(whole),
    }?;
    second.checked_add(fraction)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pull_request::application::fixtures::at;

    #[test]
    fn a_utc_timestamp_names_its_instant() {
        assert_eq!(instant("2026-01-15T12:00:00Z"), Some(at(1_768_478_400)));
    }

    #[test]
    fn an_offset_or_a_fraction_names_the_same_instant() {
        assert_eq!(
            instant("2026-01-15T13:00:00.250+01:00"),
            Some(at(1_768_478_400) + Duration::from_millis(250))
        );
    }

    #[test]
    fn a_timestamp_before_the_epoch_names_its_instant() {
        assert_eq!(
            instant("1969-12-31T23:59:00Z"),
            UNIX_EPOCH.checked_sub(Duration::from_mins(1))
        );
    }

    #[test]
    fn a_value_that_is_no_timestamp_names_no_instant() {
        for value in ["", "soon", "2026-01-15", "2026-01-15 12:00:00"] {
            assert_eq!(instant(value), None, "{value:?}");
        }
    }
}
