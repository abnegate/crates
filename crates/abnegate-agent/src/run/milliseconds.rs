//! A [`Duration`] saved as whole milliseconds.
//!
//! Writing truncates anything finer than a millisecond and saturates at
//! `u64::MAX` milliseconds.

use std::time::Duration;

use serde::Deserialize;
use serde::Deserializer;
use serde::Serializer;

pub(super) fn serialize<S: Serializer>(
    duration: &Duration,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_u64(u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
}

pub(super) fn deserialize<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Duration, D::Error> {
    u64::deserialize(deserializer).map(Duration::from_millis)
}
