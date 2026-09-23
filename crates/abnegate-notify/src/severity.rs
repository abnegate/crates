//! How loud a notification is.

use std::fmt;

use serde::{Deserialize, Serialize};

/// The weight of a notification, which each backend renders in its own way.
///
/// Closed, unlike [`Channel`](crate::Channel): a backend has to map every
/// severity to a colour or an icon, and a variant it has never heard of would
/// have no rendering at all.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    #[default]
    Info,
    Success,
    Warning,
    Error,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Success => "success",
            Self::Warning => "warning",
            Self::Error => "error",
        }
    }
}

impl fmt::Display for Severity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn info_is_the_default() {
        assert_eq!(Severity::default(), Severity::Info);
    }

    #[test]
    fn severities_are_ordered_by_weight() {
        assert!(Severity::Error > Severity::Warning);
        assert!(Severity::Warning > Severity::Success);
        assert!(Severity::Success > Severity::Info);
    }

    #[test]
    fn serde_uses_lowercase_names() {
        assert_eq!(
            serde_json::to_string(&Severity::Warning).expect("serialize"),
            r#""warning""#
        );
        assert_eq!(
            serde_json::from_str::<Severity>(r#""error""#).expect("deserialize"),
            Severity::Error
        );
    }
}
