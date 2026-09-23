use crate::client::RequestOptions;
use crate::modality::ResponseFormat;
use crate::wire::Message;
use crate::wire::ToolDefinition;

/// The request a [`StubProvider`](super::StubProvider) was last handed.
#[derive(Debug, Clone)]
pub struct Seen {
    pub model: String,
    pub messages: Vec<Message>,
    pub tools: Option<Vec<ToolDefinition>>,
    pub options: RequestOptions,
    pub response_format: Option<ResponseFormat>,
    pub temperature: Option<f32>,
}
