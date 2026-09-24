use serde::Deserialize;
use serde::Serialize;

/// Asks a runner whether it is alive; it answers with a `Pong` carrying the
/// same [`id`](Self::id).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[non_exhaustive]
pub struct Ping {
    /// Echoed back in the answer
    pub id: String,
}

impl Ping {
    /// A ping the answer to which carries `id`.
    pub fn new(id: impl Into<String>) -> Self {
        Self { id: id.into() }
    }
}
