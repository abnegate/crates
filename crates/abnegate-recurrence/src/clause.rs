//! The clauses a rule may carry.

use std::fmt;

/// One clause of the accepted subset, named in [`Error`](crate::Error) when a
/// rule is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Clause {
    /// `FREQ`, the cadence the rule repeats on.
    Frequency,
    /// `INTERVAL`, how many cadences pass between periods.
    Interval,
    /// `BYDAY`, the weekdays a weekly rule fires on.
    Day,
    /// `BYHOUR`, the hours of the day the rule fires at.
    Hour,
    /// `BYMINUTE`, the minutes of the hour the rule fires at.
    Minute,
    /// `BYMONTHDAY`, the days of the month a monthly rule fires on.
    MonthDay,
    /// `UNTIL`, the last instant the rule may fire at.
    Until,
    /// `COUNT`, how many times the rule fires in all.
    Count,
}

impl Clause {
    /// The clause's name as a rule spells it, such as `BYMONTHDAY`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Frequency => "FREQ",
            Self::Interval => "INTERVAL",
            Self::Day => "BYDAY",
            Self::Hour => "BYHOUR",
            Self::Minute => "BYMINUTE",
            Self::MonthDay => "BYMONTHDAY",
            Self::Until => "UNTIL",
            Self::Count => "COUNT",
        }
    }

    pub(crate) fn parse(name: &str) -> Option<Self> {
        match name {
            "FREQ" => Some(Self::Frequency),
            "INTERVAL" => Some(Self::Interval),
            "BYDAY" => Some(Self::Day),
            "BYHOUR" => Some(Self::Hour),
            "BYMINUTE" => Some(Self::Minute),
            "BYMONTHDAY" => Some(Self::MonthDay),
            "UNTIL" => Some(Self::Until),
            "COUNT" => Some(Self::Count),
            _ => None,
        }
    }
}

impl fmt::Display for Clause {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}
