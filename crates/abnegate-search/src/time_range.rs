//! Recency filter for a metasearch query.

use std::fmt;

use serde::Deserialize;
use serde::Serialize;

/// How far back a search may reach. The prompt tells the model to re-search
/// narrowed to a day, week or month when its sources come back stale.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum TimeRange {
    Day,
    Week,
    Month,
}

impl TimeRange {
    /// Wire name of the filter, in both the engine query and the tool schema.
    pub const PARAMETER: &'static str = "time_range";

    /// Every variant, narrowest first, for schemas and error messages.
    pub const ALL: [Self; 3] = [Self::Day, Self::Week, Self::Month];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Day => "day",
            Self::Week => "week",
            Self::Month => "month",
        }
    }
}

impl fmt::Display for TimeRange {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_wire_form_is_the_lowercase_variant_name() {
        for range in TimeRange::ALL {
            assert_eq!(
                serde_json::to_value(range).expect("serialize"),
                json!(range.as_str()),
                "{range:?} serializes to something other than its own string"
            );
            assert_eq!(range.to_string(), range.as_str());
        }
        assert_eq!(json!(TimeRange::ALL), json!(["day", "week", "month"]));
    }

    #[test]
    fn only_the_three_advertised_values_parse() {
        for range in TimeRange::ALL {
            assert_eq!(
                serde_json::from_value::<TimeRange>(json!(range.as_str())).expect("parse"),
                range
            );
        }
        for rejected in ["year", "Day", "hour", "all", ""] {
            assert!(
                serde_json::from_value::<TimeRange>(json!(rejected)).is_err(),
                "{rejected} parsed as a time range"
            );
        }
    }
}
