use serde::Serialize;

/// The body `/api/embeddings` takes, its fields in a fixed order so the
/// bytes sent never change.
#[derive(Debug, Serialize)]
pub(super) struct Request<'a> {
    pub(super) model: &'a str,
    pub(super) prompt: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) keep_alive: Option<&'a str>,
}
