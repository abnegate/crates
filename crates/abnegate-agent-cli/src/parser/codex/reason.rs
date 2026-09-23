use serde::Deserialize;

/// The nested error a `turn.failed` event may carry.
#[derive(Debug, Deserialize)]
pub struct Reason {
    #[serde(default)]
    pub message: Option<String>,
}
