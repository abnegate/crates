use serde::Deserialize;
use std::fmt;

/// The side of a pull request's diff a review thread was left on, as GitHub
/// names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
#[non_exhaustive]
pub enum DiffSide {
    /// The base side: lines as they were before the change, including the
    /// lines it deletes.
    Left,
    /// The head side: lines as the change leaves them, including the lines it
    /// adds.
    Right,
    /// GitHub gave no side, or one this crate does not know yet.
    #[default]
    #[serde(other)]
    Unknown,
}

impl DiffSide {
    /// The side as GitHub spells it, or `UNKNOWN`.
    pub fn as_str(self) -> &'static str {
        match self {
            DiffSide::Left => "LEFT",
            DiffSide::Right => "RIGHT",
            DiffSide::Unknown => "UNKNOWN",
        }
    }
}

impl fmt::Display for DiffSide {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_diff_side_reads_and_prints_as_github_spells_it() {
        for (side, spelled) in [
            (DiffSide::Left, "LEFT"),
            (DiffSide::Right, "RIGHT"),
            (DiffSide::Unknown, "UNKNOWN"),
        ] {
            assert_eq!(
                serde_json::from_value::<DiffSide>(json!(spelled)).unwrap(),
                side
            );
            assert_eq!(side.as_str(), spelled);
            assert_eq!(side.to_string(), spelled);
        }
    }

    #[test]
    fn a_diff_side_github_has_not_named_yet_is_unknown() {
        assert_eq!(
            serde_json::from_value::<DiffSide>(json!("CENTER")).unwrap(),
            DiffSide::Unknown
        );
        assert_eq!(DiffSide::default(), DiffSide::Unknown);
    }
}
