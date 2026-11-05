use serde::Deserialize;

/// The body `/api/embeddings` answers with.
#[derive(Debug, Deserialize)]
pub(super) struct Response {
    pub(super) embedding: Vec<f32>,
}
