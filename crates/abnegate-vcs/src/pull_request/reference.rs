/// Where a pull request lives, recovered from the URL a run recorded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullRequestReference {
    pub owner: String,
    pub repository: String,
    pub number: i64,
}
