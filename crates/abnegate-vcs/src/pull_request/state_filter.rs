/// Which issues or pull requests a list holds, by whether they are open.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum StateFilter {
    /// Only those still open.
    #[default]
    Open,
    /// Only those closed, merged or not.
    Closed,
    /// Every one, open or closed.
    All,
}

impl StateFilter {
    /// The word GitHub's `state` parameter takes: `open`, `closed` or `all`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Closed => "closed",
            Self::All => "all",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_filter_is_the_word_github_takes() {
        for (filter, word) in [
            (StateFilter::Open, "open"),
            (StateFilter::Closed, "closed"),
            (StateFilter::All, "all"),
        ] {
            assert_eq!(filter.as_str(), word);
        }
    }
}
