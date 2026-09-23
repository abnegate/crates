use serde::Deserialize;

use crate::wire::stream_function_call::StreamFunctionCall;

/// A tool call arriving one fragment at a time.
#[derive(Debug, Clone, Deserialize)]
pub struct StreamToolCall {
    pub index: u32,
    pub id: Option<String>,
    #[serde(rename = "type")]
    pub call_type: Option<String>,
    pub function: Option<StreamFunctionCall>,
}
