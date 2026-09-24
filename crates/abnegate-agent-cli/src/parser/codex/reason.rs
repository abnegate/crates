use serde::Deserialize;

/// The nested error a `turn.failed` event may carry.
#[derive(Debug, Deserialize)]
pub(crate) struct Reason {
    #[serde(default)]
    pub(crate) message: Option<String>,
}
