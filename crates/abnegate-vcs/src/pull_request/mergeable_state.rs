use serde::Deserialize;

/// What stands between a pull request and a merge, as GitHub sums it up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum MergeableState {
    /// The head is behind its base and has to be brought up to date first.
    Behind,
    /// A branch protection rule, such as a required review or check, blocks
    /// it.
    Blocked,
    /// Nothing: it merges cleanly and its checks pass.
    Clean,
    /// The head conflicts with its base.
    Dirty,
    /// It is a draft.
    Draft,
    /// Nothing, and a pre-receive hook will run when it merges.
    HasHooks,
    /// Nothing required, but a check that is not required has failed.
    Unstable,
    /// GitHub has not worked it out yet, or named something this crate does
    /// not know.
    #[default]
    #[serde(other)]
    Unknown,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_mergeable_state_reads_as_github_spells_it() {
        for (spelled, state) in [
            ("behind", MergeableState::Behind),
            ("blocked", MergeableState::Blocked),
            ("clean", MergeableState::Clean),
            ("dirty", MergeableState::Dirty),
            ("draft", MergeableState::Draft),
            ("has_hooks", MergeableState::HasHooks),
            ("unstable", MergeableState::Unstable),
            ("unknown", MergeableState::Unknown),
        ] {
            assert_eq!(
                serde_json::from_value::<MergeableState>(json!(spelled)).unwrap(),
                state,
                "{spelled}"
            );
        }
    }
}
