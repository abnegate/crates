//! A [`Duration`] carried on the wire as whole milliseconds.
//!
//! Writing truncates anything finer than a millisecond and saturates at
//! `u64::MAX` milliseconds, which is longer than any process lives.

use std::time::Duration;

use serde::Deserialize;
use serde::Deserializer;
use serde::Serializer;

pub(crate) fn serialize_optional<S: Serializer>(
    duration: &Option<Duration>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    match duration {
        Some(duration) => serializer.serialize_some(&whole(*duration)),
        None => serializer.serialize_none(),
    }
}

pub(crate) fn deserialize_optional<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Duration>, D::Error> {
    Option::<u64>::deserialize(deserializer)
        .map(|milliseconds| milliseconds.map(Duration::from_millis))
}

fn whole(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use serde::Serialize;

    use super::*;

    #[derive(Serialize, Deserialize, PartialEq, Debug)]
    struct Carrier {
        #[serde(
            default,
            serialize_with = "serialize_optional",
            deserialize_with = "deserialize_optional"
        )]
        timeout: Option<Duration>,
    }

    #[test]
    fn a_timeout_is_written_as_whole_milliseconds() {
        let carrier = Carrier {
            timeout: Some(Duration::from_micros(300_000_999)),
        };

        assert_eq!(
            serde_json::to_string(&carrier).unwrap(),
            r#"{"timeout":300000}"#
        );
    }

    #[test]
    fn an_absent_timeout_is_written_as_null_and_read_back_from_null_or_nothing() {
        let carrier = Carrier { timeout: None };

        assert_eq!(
            serde_json::to_string(&carrier).unwrap(),
            r#"{"timeout":null}"#
        );
        for line in [r#"{"timeout":null}"#, "{}"] {
            assert_eq!(serde_json::from_str::<Carrier>(line).unwrap(), carrier);
        }
    }

    #[test]
    fn the_largest_value_round_trips_and_a_longer_duration_saturates() {
        let largest = format!(r#"{{"timeout":{}}}"#, u64::MAX);
        let carrier: Carrier = serde_json::from_str(&largest).unwrap();

        assert_eq!(carrier.timeout, Some(Duration::from_millis(u64::MAX)));
        assert_eq!(serde_json::to_string(&carrier).unwrap(), largest);
        assert_eq!(
            serde_json::to_string(&Carrier {
                timeout: Some(Duration::MAX),
            })
            .unwrap(),
            largest
        );
    }

    #[test]
    fn a_negative_or_fractional_value_is_refused() {
        for line in [r#"{"timeout":-1}"#, r#"{"timeout":1.5}"#] {
            assert!(serde_json::from_str::<Carrier>(line).is_err(), "{line}");
        }
    }
}
