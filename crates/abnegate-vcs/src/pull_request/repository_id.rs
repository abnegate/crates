use std::fmt;

/// GitHub's node identifier for a repository, which stays the same when the
/// repository is renamed or transferred, as
/// [`PullRequestService::list_open_pull_requests`](crate::pull_request::PullRequestService::list_open_pull_requests)
/// reads it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RepositoryId(String);

impl RepositoryId {
    /// The repository GitHub's GraphQL API identifies as `id`.
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// The node identifier as GitHub spells it.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RepositoryId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_repository_id_reads_back_as_github_spelled_it() {
        let id = RepositoryId::new("R_kgDOAcme");

        assert_eq!(id.as_str(), "R_kgDOAcme");
        assert_eq!(id.to_string(), "R_kgDOAcme");
    }
}
