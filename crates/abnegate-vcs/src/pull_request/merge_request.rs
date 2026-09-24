use crate::pull_request::MergeMethod;
use serde::Serialize;

/// The body of a request to merge a pull request: the commit it makes, the
/// head it expects, and how the commits land.
#[derive(Debug, Clone, Serialize)]
pub(super) struct MergeRequest<'a> {
    pub(super) commit_title: &'a str,
    pub(super) commit_message: &'a str,
    pub(super) sha: &'a str,
    pub(super) merge_method: MergeMethod,
}
