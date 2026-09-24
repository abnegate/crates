use crate::client::RequestOptions;
use crate::modality::ResponseFormat;
use crate::wire::Message;
use crate::wire::ToolDefinition;

/// One completion to run.
///
/// Borrowed for the same reason [`ChatRequest`](crate::ChatRequest) is: the
/// agent loop already owns the conversation and the tool definitions, and a
/// provider that took them by value would clone the whole history once per
/// turn.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct CompletionRequest<'a> {
    pub model: &'a str,
    pub messages: &'a [Message],
    pub tools: Option<&'a [ToolDefinition]>,
    pub options: RequestOptions,
    /// The shape the answer must take. Only a provider whose
    /// [`Capabilities::structured_output`](crate::Capabilities::structured_output)
    /// is set promises to honour it.
    pub response_format: Option<&'a ResponseFormat>,
    /// A sampling temperature for this request in place of the provider's own.
    pub temperature: Option<f32>,
}

impl<'a> CompletionRequest<'a> {
    pub fn new(model: &'a str, messages: &'a [Message], options: RequestOptions) -> Self {
        Self {
            model,
            messages,
            tools: None,
            options,
            response_format: None,
            temperature: None,
        }
    }

    pub fn with_tools(mut self, tools: &'a [ToolDefinition]) -> Self {
        self.tools = Some(tools);
        self
    }

    pub fn with_response_format(mut self, response_format: &'a ResponseFormat) -> Self {
        self.response_format = Some(response_format);
        self
    }

    pub fn with_temperature(mut self, temperature: f32) -> Self {
        self.temperature = Some(temperature);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_asks_for_nothing_beyond_what_it_is_given() {
        let messages = [Message::user("hi")];
        let request = CompletionRequest::new("qwen3", &messages, RequestOptions::new(64));

        assert_eq!(request.model, "qwen3");
        assert!(request.tools.is_none());
        assert!(request.response_format.is_none());
        assert!(request.temperature.is_none());
        assert_eq!(request.options.reserved, 64);
    }

    #[test]
    fn a_request_carries_every_setting_it_is_given() {
        let messages = [Message::user("hi")];
        let tools = [ToolDefinition::function(
            "read_file",
            "Read a file",
            serde_json::json!({"type": "object"}),
        )];
        let format = ResponseFormat::Json {
            schema: None,
            strict: false,
        };

        let request = CompletionRequest::new("qwen3", &messages, RequestOptions::new(64))
            .with_tools(&tools)
            .with_response_format(&format)
            .with_temperature(0.0);

        assert_eq!(request.tools.map(<[ToolDefinition]>::len), Some(1));
        assert!(matches!(
            request.response_format,
            Some(ResponseFormat::Json { .. })
        ));
        assert_eq!(request.temperature, Some(0.0));
    }
}
