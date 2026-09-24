use serde::Serialize;

/// The body of a request to comment on a pull request's conversation, or to
/// reply in a review thread.
#[derive(Debug, Clone, Serialize)]
pub(super) struct CommentRequest<'a> {
    pub(super) body: &'a str,
}
