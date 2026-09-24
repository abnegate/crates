use crate::pull_request::ReviewEvent;
use serde::Serialize;

/// The body of a request to submit a review of a pull request.
#[derive(Debug, Clone, Serialize)]
pub(super) struct ReviewRequest<'a> {
    pub(super) commit_id: &'a str,
    pub(super) body: &'a str,
    pub(super) event: ReviewEvent,
}
