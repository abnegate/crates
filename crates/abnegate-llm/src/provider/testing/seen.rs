use crate::client::RequestOptions;
use crate::modality::ResponseFormat;
use crate::wire::Message;
use crate::wire::ToolDefinition;

/// The request a [`StubProvider`](super::StubProvider) was last handed.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Seen {
    /// The model asked for.
    pub model: String,
    /// The conversation.
    pub messages: Vec<Message>,
    /// The tools offered, if any.
    pub tools: Option<Vec<ToolDefinition>>,
    /// The output reservation.
    pub options: RequestOptions,
    /// The response format asked for, if any.
    pub response_format: Option<ResponseFormat>,
    /// The temperature asked for, if any.
    pub temperature: Option<f32>,
}
