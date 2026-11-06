use serde::Deserialize;
use std::fmt;

/// Which of an account's repositories a GitHub App installation can reach,
/// as GitHub names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum RepositorySelection {
    /// Every repository the account owns, those it creates later included.
    All,
    /// Only the repositories the account chose when installing, or since.
    Selected,
    /// A selection GitHub gave that this crate does not know yet.
    #[serde(other)]
    Unknown,
}

impl RepositorySelection {
    /// The selection as GitHub spells it, or `unknown`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Selected => "selected",
            Self::Unknown => "unknown",
        }
    }
}

impl fmt::Display for RepositorySelection {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_repository_selection_reads_and_prints_as_github_spells_it() {
        for (selection, spelled) in [
            (RepositorySelection::All, "all"),
            (RepositorySelection::Selected, "selected"),
            (RepositorySelection::Unknown, "unknown"),
        ] {
            assert_eq!(
                serde_json::from_value::<RepositorySelection>(json!(spelled)).unwrap(),
                selection
            );
            assert_eq!(selection.as_str(), spelled);
            assert_eq!(selection.to_string(), spelled);
        }
    }

    #[test]
    fn a_selection_github_has_not_named_yet_is_unknown() {
        assert_eq!(
            serde_json::from_value::<RepositorySelection>(json!("none")).unwrap(),
            RepositorySelection::Unknown
        );
    }
}
