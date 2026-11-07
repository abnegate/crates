//! A weekday ordered the way a week runs.

use chrono::Weekday;
use std::cmp::Ordering;

/// A `BYDAY` value, ordered Monday first.
///
/// [`Weekday`] has no `Ord`, and a clause list is sorted and deduplicated on
/// the week's own order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Day(pub(crate) Weekday);

impl Day {
    /// Reads a two-letter weekday, `MO` to `SU`, already upper-cased.
    pub(crate) fn parse(raw: &str) -> Option<Self> {
        let weekday = match raw {
            "MO" => Weekday::Mon,
            "TU" => Weekday::Tue,
            "WE" => Weekday::Wed,
            "TH" => Weekday::Thu,
            "FR" => Weekday::Fri,
            "SA" => Weekday::Sat,
            "SU" => Weekday::Sun,
            _ => return None,
        };
        Some(Self(weekday))
    }
}

impl Ord for Day {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0
            .num_days_from_monday()
            .cmp(&other.0.num_days_from_monday())
    }
}

impl PartialOrd for Day {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
