use crate::pull_request::graphql_checked_commit::GraphQlCheckedCommit;
use serde::Deserialize;

/// One entry in a pull request's commits, as GitHub's GraphQL API answers for
/// it, holding the commit itself.
#[derive(Debug, Clone, Deserialize)]
pub(super) struct GraphQlCommitNode {
    pub(super) commit: GraphQlCheckedCommit,
}
