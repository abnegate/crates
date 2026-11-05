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

    /// The method GitHub's GraphQL API names `name`, if it is one this crate
    /// knows.
    pub(super) fn from_graphql(name: &str) -> Option<Self> {
        [Self::Squash, Self::Merge, Self::Rebase]
            .into_iter()
            .find(|method| method.graphql() == name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_merge_method_reads_back_as_graphql_names_it() {
        for method in [MergeMethod::Squash, MergeMethod::Merge, MergeMethod::Rebase] {
            assert_eq!(MergeMethod::from_graphql(method.graphql()), Some(method));
        }
        assert_eq!(MergeMethod::from_graphql("FAST_FORWARD"), None);
        assert_eq!(MergeMethod::from_graphql("squash"), None);
    }
}
