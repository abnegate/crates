use serde::Deserialize;

use crate::wire::stream_delta::StreamDelta;

/// One streaming choice.
#[derive(Debug, Clone, Deserialize)]
#[non_exhaustive]
pub struct StreamChoice {
    #[serde(default)]
    pub index: u32,
    pub delta: StreamDelta,
    pub finish_reason: Option<String>,
}
