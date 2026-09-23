use serde::Serialize;

/// Request to create a pull request
#[derive(Debug, Clone, Serialize)]
pub(super) struct CreatePrRequest {
    pub(super) title: String,
    pub(super) body: String,
    pub(super) head: String,
    pub(super) base: String,
    pub(super) draft: bool,
}
