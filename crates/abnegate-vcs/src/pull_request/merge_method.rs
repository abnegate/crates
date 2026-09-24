use serde::Serialize;

/// How a pull request's commits land on its base.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum MergeMethod {
    /// As one commit holding every change.
    Squash,
    /// As they are, joined by a merge commit.
    Merge,
    /// Each replayed onto the base, with no merge commit.
    Rebase,
}

impl MergeMethod {
    /// The name GitHub's GraphQL API gives the method.
    pub(super) fn graphql(self) -> &'static str {
        match self {
            Self::Squash => "SQUASH",
            Self::Merge => "MERGE",
            Self::Rebase => "REBASE",
        }
    }
}
