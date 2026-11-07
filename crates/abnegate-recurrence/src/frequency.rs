//! The cadences a rule may repeat on.

use chrono::TimeDelta;
use std::fmt;

pub(crate) const SHORTEST_MONTH_DAYS: u32 = 28;

/// The cadence a rule repeats on, as its `FREQ` names it. Hourly is the
/// finest cadence a rule can state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Frequency {
    /// `FREQ=HOURLY`.
    Hourly,
    /// `FREQ=DAILY`.
    Daily,
    /// `FREQ=WEEKLY`.
    Weekly,
    /// `FREQ=MONTHLY`.
    Monthly,
}

impl Frequency {
    /// The cadence's name as an error spells it, such as `Monthly`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Hourly => "Hourly",
            Self::Daily => "Daily",
            Self::Weekly => "Weekly",
            Self::Monthly => "Monthly",
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "HOURLY" => Some(Self::Hourly),
            "DAILY" => Some(Self::Daily),
            "WEEKLY" => Some(Self::Weekly),
            "MONTHLY" => Some(Self::Monthly),
            _ => None,
        }
    }

    /// The period at `INTERVAL=1`. A month is measured at its shortest, so a
    /// bound read from it holds for February as well as for July.
    pub(crate) fn shortest(self) -> TimeDelta {
        match self {
            Self::Hourly => TimeDelta::hours(1),
            Self::Daily => TimeDelta::days(1),
            Self::Weekly => TimeDelta::weeks(1),
            Self::Monthly => TimeDelta::days(i64::from(SHORTEST_MONTH_DAYS)),
        }
    }
}

impl fmt::Display for Frequency {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}
