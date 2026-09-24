use serde::Deserialize;

/// The nested error a `turn.failed` event may carry.
#[derive(Debug, Deserialize)]
#[non_exhaustive]
pub struct Reason {
    #[serde(default)]
    pub message: Option<String>,
}
