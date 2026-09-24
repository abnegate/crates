//! The OpenAI-compatible chat completions client.

mod config;
mod events;
mod options;
mod pool;

use std::time::Duration;

use abnegate_secret::redact;
use futures::Stream;
use futures::StreamExt;
use reqwest::Client;
use reqwest::Url;

pub use crate::client::config::LlmConfig;
pub use crate::client::options::RequestOptions;
pub(crate) use crate::client::pool::Pool;

use crate::client::events::EventDecoder;
use crate::error::Error;
use crate::provider::CompletionRequest;
use crate::reasoning::Effort;
use crate::wire::ChatRequest;
use crate::wire::ChatResponse;
use crate::wire::ChatStreamChunk;
use crate::wire::Message;
use crate::wire::ToolDefinition;

/// A client for an OpenAI-compatible chat completions endpoint.
///
/// A non-streaming completion must finish within [`LlmConfig::timeout`]; a
/// streamed one may run as long as it keeps sending, but fails once it goes
/// [`LlmConfig::read_timeout`] without a byte.
#[derive(Debug, Clone)]
pub struct LlmClient {
    client: Client,
    config: LlmConfig,
    stop: Vec<String>,
    ollama: Option<(String, u64)>,
    /// Alias and resolved effort. Summarization clones the client and clears this.
    reasoning: Option<(String, Effort)>,
}

impl LlmClient {
    pub fn new(config: LlmConfig) -> Self {
        Self {
            client: Pool::client(),
            config,
            stop: Vec::new(),
            ollama: None,
            reasoning: None,
        }
    }

    /// Ask the provider to halt on these strings. Custom GGUFs often ignore
    /// their own end tokens unless the request repeats them.
    pub fn with_stop(mut self, stop: Vec<String>) -> Self {
        self.stop = stop;
        self
    }

    /// Derive a client with a task-specific sampling temperature.
    pub fn with_temperature(mut self, temperature: f32) -> Self {
        self.config.temperature = temperature;
        self
    }

    /// Set a verified Ollama route's context capacity. The alias guard prevents
    /// forwarding provider-specific options after changing models; `num_ctx` is
    /// accepted as a top-level non-OpenAI option.
    pub fn with_ollama_context(mut self, model: impl Into<String>, limit: u64) -> Self {
        self.ollama = Some((model.into(), limit));
        self
    }

    /// Enable thinking for this alias. `reasoning_effort` maps onto Ollama
    /// `think`, Anthropic extended thinking, and OpenAI o-series.
    pub fn with_reasoning(mut self, model: impl Into<String>, effort: Effort) -> Self {
        self.reasoning = Some((model.into(), effort));
        self
    }

    /// Context compaction must stay a cheap structured rewrite.
    pub fn without_reasoning(mut self) -> Self {
        self.reasoning = None;
        self
    }

    pub fn config(&self) -> &LlmConfig {
        &self.config
    }

    fn body(&self, request: ChatRequest<'_>) -> Result<serde_json::Value, Error> {
        let mut body = serde_json::to_value(&request)?;
        if let Some((model, limit)) = &self.ollama
            && model == request.model
        {
            body["num_ctx"] = (*limit).into();
        }
        if let Some((model, effort)) = &self.reasoning
            && model == request.model
        {
            body["reasoning_effort"] = effort.as_str().into();
        }
        if request.stream == Some(true) {
            body["stream_options"] = serde_json::json!({ "include_usage": true });
        }
        Ok(body)
    }

    fn validate(&self, url: &str) -> Result<(), Error> {
        let parsed = Url::parse(url).map_err(|_| {
            Error::InvalidConfig("LLM base_url must be a valid absolute URL".to_string())
        })?;

        match parsed.scheme() {
            "http" | "https" => {}
            _ => {
                return Err(Error::InvalidConfig(
                    "LLM base_url must use http or https".to_string(),
                ));
            }
        }

        if !parsed.username().is_empty() || parsed.password().is_some() {
            return Err(Error::InvalidConfig(
                "LLM base_url must not include userinfo".to_string(),
            ));
        }

        if parsed.host_str().is_none() {
            return Err(Error::InvalidConfig(
                "LLM base_url must include a host".to_string(),
            ));
        }

        // No private-network rule here on purpose. Every caller passes an
        // operator-configured host, and a self-hosted deployment points at
        // loopback, a LAN address or a compose service name. The check belongs
        // where a tenant-supplied host is first accepted, against that value.
        Ok(())
    }

    async fn send(&self, url: &str, body: &serde_json::Value) -> Result<reqwest::Response, Error> {
        self.validate(url)?;
        let mut request = self.client.post(url).json(body);
        if !self.config.api_key.is_empty() {
            request = request.bearer_auth(self.config.api_key.expose());
        }
        Ok(request.send().await?)
    }

    async fn dispatch(&self, request: ChatRequest<'_>) -> Result<reqwest::Response, Error> {
        let url = format!("{}/chat/completions", self.config.base_url);
        let response = self.send(&url, &self.body(request)?).await?;

        let status = response.status();
        if status.is_success() {
            return Ok(response);
        }

        let body = response.text().await.unwrap_or_default();
        Err(Error::Api {
            status: status.as_u16(),
            message: redact(&body).into_owned(),
        })
    }

    fn request<'a>(
        &'a self,
        model: &'a str,
        messages: &'a [Message],
        tools: Option<&'a [ToolDefinition]>,
        options: RequestOptions,
        stream: bool,
    ) -> ChatRequest<'a> {
        ChatRequest {
            model,
            messages,
            tools,
            tool_choice: None,
            temperature: Some(self.config.temperature),
            maximum_tokens: Some(options.reserved),
            stream: Some(stream),
            stop: (!self.stop.is_empty()).then_some(self.stop.as_slice()),
            response_format: None,
        }
    }

    fn reserved(&self) -> RequestOptions {
        RequestOptions {
            reserved: self.config.maximum_tokens,
        }
    }

    /// Make a chat completion request against the configured model.
    pub async fn chat(
        &self,
        messages: &[Message],
        tools: Option<&[ToolDefinition]>,
    ) -> Result<ChatResponse, Error> {
        self.chat_with_model(&self.config.default_model, messages, tools)
            .await
    }

    /// Make a chat completion request against a specific model.
    pub async fn chat_with_model(
        &self,
        model: &str,
        messages: &[Message],
        tools: Option<&[ToolDefinition]>,
    ) -> Result<ChatResponse, Error> {
        self.chat_with_options(model, messages, tools, self.reserved())
            .await
    }

    pub async fn chat_with_options(
        &self,
        model: &str,
        messages: &[Message],
        tools: Option<&[ToolDefinition]>,
        options: RequestOptions,
    ) -> Result<ChatResponse, Error> {
        self.execute(self.request(model, messages, tools, options, false))
            .await
    }

    /// Make a chat completion request exactly as a
    /// [`CompletionProvider`](crate::CompletionProvider) was asked for it,
    /// with its response format and any temperature override.
    pub async fn chat_with_request(
        &self,
        request: CompletionRequest<'_>,
    ) -> Result<ChatResponse, Error> {
        let mut chat = self.request(
            request.model,
            request.messages,
            request.tools,
            request.options,
            false,
        );
        chat.response_format = request.response_format;
        if let Some(temperature) = request.temperature {
            chat.temperature = Some(temperature);
        }
        self.execute(chat).await
    }

    async fn execute(&self, request: ChatRequest<'_>) -> Result<ChatResponse, Error> {
        let timeout = self.config.timeout;
        tokio::time::timeout(timeout, async {
            let response = self.dispatch(request).await?;
            Ok(response.json().await?)
        })
        .await
        .map_err(|_| Error::Timeout(timeout))?
    }

    /// Stream a chat completion from the configured model.
    pub async fn chat_stream(
        &self,
        messages: &[Message],
        tools: Option<&[ToolDefinition]>,
    ) -> Result<impl Stream<Item = Result<ChatStreamChunk, Error>> + use<>, Error> {
        self.chat_stream_with_model(&self.config.default_model, messages, tools)
            .await
    }

    /// Stream a chat completion from a specific model.
    pub async fn chat_stream_with_model(
        &self,
        model: &str,
        messages: &[Message],
        tools: Option<&[ToolDefinition]>,
    ) -> Result<impl Stream<Item = Result<ChatStreamChunk, Error>> + use<>, Error> {
        self.chat_stream_with_options(model, messages, tools, self.reserved())
            .await
    }

    pub async fn chat_stream_with_options(
        &self,
        model: &str,
        messages: &[Message],
        tools: Option<&[ToolDefinition]>,
        options: RequestOptions,
    ) -> Result<impl Stream<Item = Result<ChatStreamChunk, Error>> + use<>, Error> {
        let read_timeout = self.config.read_timeout;
        let response = tokio::time::timeout(
            read_timeout,
            self.dispatch(self.request(model, messages, tools, options, true)),
        )
        .await
        .map_err(|_| Error::Timeout(read_timeout))??;

        Ok(events(response, read_timeout))
    }
}

fn events(
    response: reqwest::Response,
    read_timeout: Duration,
) -> impl Stream<Item = Result<ChatStreamChunk, Error>> {
    async_stream::stream! {
        let mut bytes = response.bytes_stream();
        let mut decoder = EventDecoder::default();

        while !decoder.is_finished() {
            let Ok(next) = tokio::time::timeout(read_timeout, bytes.next()).await else {
                yield Err(Error::Timeout(read_timeout));
                break;
            };
            match next {
                Some(Ok(chunk)) => {
                    for decoded in decoder.push(&chunk) {
                        yield decoded;
                    }
                }
                Some(Err(error)) => {
                    yield Err(Error::from(error));
                    break;
                }
                None => {
                    for decoded in decoder.finish() {
                        yield decoded;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
