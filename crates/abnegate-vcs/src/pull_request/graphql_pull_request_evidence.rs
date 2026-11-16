use crate::pull_request::graphql_commit_node::GraphQlCommitNode;
use crate::pull_request::graphql_connection::GraphQlConnection;
use crate::pull_request::graphql_counted_connection::GraphQlCountedConnection;
use crate::pull_request::graphql_issue_comment::GraphQlIssueComment;
use crate::pull_request::graphql_listed_pull_request::GraphQlListedPullRequest;
use crate::pull_request::graphql_node::GraphQlNode;
use crate::pull_request::graphql_thread_resolution::GraphQlThreadResolution;
use serde::Deserialize;

/// A pull request as GitHub's GraphQL API answers for it with everything that
/// says whether it is ready to merge: what stands in its way, its review
/// threads, its newest conversation comments, and the checks on its head.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct GraphQlPullRequestEvidence {
    #[serde(flatten)]
    pub(super) listed: GraphQlListedPullRequest,
    pub(super) repository: GraphQlNode,
    #[serde(default)]
    pub(super) merge_state_status: Option<String>,
    pub(super) review_threads: GraphQlConnection<GraphQlThreadResolution>,
    pub(super) comments: GraphQlCountedConnection<GraphQlIssueComment>,
    pub(super) commits: GraphQlConnection<GraphQlCommitNode>,
}
