use serde::Deserialize;

use crate::wire::stream_delta::StreamDelta;

/// One streaming choice.
#[derive(Debug, Clone, Deserialize)]
#[non_exhaustive]
pub struct StreamChoice {
    /// Which answer this adds to, from 0.
    #[serde(default)]
    pub index: u32,
    /// What it adds.
    pub delta: StreamDelta,
    /// Why the model stopped, on the chunk that ends the answer.
    pub finish_reason: Option<String>,
}
