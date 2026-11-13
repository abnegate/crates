/// Whether a release is marked as the repository's latest when it is created.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum MakeLatest {
    /// Mark it as the latest release.
    True,
    /// Leave the latest release as it is.
    False,
    /// Mark it as the latest when it is the newest full release by creation
    /// date and semantic version, as GitHub decides.
    Legacy,
}

impl MakeLatest {
    /// The word GitHub's `make_latest` field takes: `true`, `false` or
    /// `legacy`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::True => "true",
            Self::False => "false",
            Self::Legacy => "legacy",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_choice_is_the_word_github_takes() {
        for (choice, word) in [
            (MakeLatest::True, "true"),
            (MakeLatest::False, "false"),
            (MakeLatest::Legacy, "legacy"),
        ] {
            assert_eq!(choice.as_str(), word);
        }
    }
}
