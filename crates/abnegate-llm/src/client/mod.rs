//! The OpenAI-compatible chat completions client.

mod config;
mod events;
mod options;
mod pool;
mod reporter;
mod trust;

use std::time::Duration;

use abnegate_http::metadata;
use futures::Stream;
use futures::StreamExt;
use reqwest::Client;
use reqwest::Url;

pub use crate::client::config::LlmConfig;
pub use crate::client::options::RequestOptions;
pub(crate) use crate::client::pool::Pool;
pub use crate::client::trust::Trust;

use crate::budget::Budget;
use crate::client::events::EventDecoder;
use crate::client::reporter::Reporter;
use crate::dialect::Dialect;
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
/// Every request is shaped for [`LlmConfig::dialect`] before it is sent: the
/// stops, temperature, output budget field, messages and tools a provider's
/// own API would refuse are adjusted, and a compatible endpoint is sent the
/// request unchanged.
///
/// A non-streaming completion must finish within [`LlmConfig::timeout`]; a
/// streamed one may run as long as it keeps sending, but fails once it goes
/// [`LlmConfig::read_timeout`] without a byte.
///
/// A refusal reaches the caller as [`Error::Api`], and an error sent in place
/// of a streamed chunk as [`Error::Stream`], without the key the client sent;
/// for an endpoint a [`Trust::Tenant`] chose, as nothing but its status and
/// the endpoint's own error message.
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
    /// A client for the endpoint `config` names. Every client in the process
    /// with the same [`LlmConfig::trust`] shares one connection pool, so
    /// building one is cheap.
    ///
    /// # Panics
    ///
    /// Panics if no HTTP client can be built at all, as when no TLS backend
    /// can be initialised, just as `reqwest::Client::new` does.
    pub fn new(config: LlmConfig) -> Self {
        Self {
            client: Pool::client(config.trust),
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
    /// accepted as a top-level non-OpenAI option, so it is sent only to a
    /// [`Dialect::Compatible`](crate::Dialect::Compatible) endpoint.
    pub fn with_ollama_context(mut self, model: impl Into<String>, limit: u64) -> Self {
        self.ollama = Some((model.into(), limit));
        self
    }

    /// Enable thinking for this alias. `reasoning_effort` maps onto Ollama
    /// `think`, Anthropic extended thinking, and OpenAI o-series; it is sent
    /// only where [`Dialect::effort`](crate::Dialect::effort) allows it.
    pub fn with_reasoning(mut self, model: impl Into<String>, effort: Effort) -> Self {
        self.reasoning = Some((model.into(), effort));
        self
    }

    /// Context compaction must stay a cheap structured rewrite.
    pub fn without_reasoning(mut self) -> Self {
        self.reasoning = None;
        self
    }

    /// The endpoint settings, with any temperature a `with_` method set.
    pub fn config(&self) -> &LlmConfig {
        &self.config
    }

    fn body(&self, request: ChatRequest<'_>) -> Result<serde_json::Value, Error> {
        let dialect = self.config.dialect;
        let model = request.model;
        let stops = dialect.stops(model, request.stop.unwrap_or_default());
        let stop = match dialect {
            Dialect::Compatible => request.stop,
            _ => (!stops.is_empty()).then_some(&*stops),
        };
        let messages = dialect.messages(request.messages);
        let tools = dialect.tools(request.tools);
        let response_format = dialect.response_format(request.response_format);
        let (maximum_tokens, maximum_completion_tokens) = budgeted(dialect, &request);
        let shaped = ChatRequest {
            messages: &messages,
            tools: tools.as_deref(),
            temperature: request
                .temperature
                .and_then(|temperature| dialect.temperature(model, temperature)),
            maximum_tokens,
            maximum_completion_tokens,
            stop,
            response_format,
            ..request
        };
        let mut body = serde_json::to_value(&shaped)?;
        if let Some((alias, limit)) = &self.ollama
            && alias == model
            && dialect.extended()
        {
            body["num_ctx"] = (*limit).into();
        }
        if let Some((alias, effort)) = &self.reasoning
            && alias == model
            && dialect.effort(model)
        {
            body["reasoning_effort"] = effort.as_str().into();
        }
        if shaped.stream == Some(true) {
            body["stream_options"] = serde_json::json!({ "include_usage": true });
        }
        Ok(body)
    }

    async fn send(&self, url: &str, body: &serde_json::Value) -> Result<reqwest::Response, Error> {
        let url = validate(url)?;
        if self.config.trust == Trust::Tenant && metadata::is_url(&url) {
            return Err(Error::InvalidConfig(
                "LLM base_url must not point at a link-local or cloud metadata address".to_string(),
            ));
        }
        let mut request = self.client.post(url).json(body);
        let key = self.config.api_key.expose().trim();
        if !key.is_empty() {
            request = request.bearer_auth(key);
        }
        Ok(request.send().await?)
    }

    async fn dispatch(&self, request: ChatRequest<'_>) -> Result<reqwest::Response, Error> {
        let url = format!("{}/chat/completions", self.config.base_url);
        let response = self.send(&url, &self.body(request)?).await?;

        if response.status().is_success() {
            return Ok(response);
        }

        Err(self.reporter().rejected(response).await)
    }

    fn reporter(&self) -> Reporter {
        Reporter::new(self.config.trust, self.config.api_key.clone())
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
            maximum_completion_tokens: None,
            stream: Some(stream),
            stop: (!self.stop.is_empty()).then_some(self.stop.as_slice()),
            response_format: None,
        }
    }

    fn reserved(&self) -> RequestOptions {
        RequestOptions::new(self.config.maximum_tokens)
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

    /// Make a chat completion request against `model`, reserving
    /// `options.reserved` tokens for the answer. Fails with
    /// [`Error::Timeout`] past [`LlmConfig::timeout`].
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

    /// Stream a chat completion from `model`, reserving `options.reserved`
    /// tokens for the answer. The stream fails with [`Error::Timeout`] when
    /// the endpoint goes [`LlmConfig::read_timeout`] without sending a byte.
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

        Ok(events(response, read_timeout, self.reporter()))
    }
}

/// The `max_tokens` and `max_completion_tokens` `request` is sent with: as the
/// caller set them for a compatible endpoint, and otherwise the budget moved
/// into the one field the model reads.
fn budgeted(dialect: Dialect, request: &ChatRequest<'_>) -> (Option<u32>, Option<u32>) {
    if dialect == Dialect::Compatible {
        return (request.maximum_tokens, request.maximum_completion_tokens);
    }
    let reserved = request.maximum_tokens.or(request.maximum_completion_tokens);
    match dialect.budget(request.model) {
        Budget::MaximumTokens => (reserved, None),
        Budget::MaximumCompletionTokens => (None, reserved),
    }
}

/// Check an outbound request URL and hand back the URL to request, so a
/// caller can only send the value that was checked.
///
/// Loopback, LAN and single-label hosts pass: a self-hosted endpoint lives
/// there. The metadata guard a [`Trust::Tenant`] endpoint needs is applied
/// by the caller, against the URL this returns.
fn validate(url: &str) -> Result<Url, Error> {
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

    Ok(parsed)
}

fn events(
    response: reqwest::Response,
    read_timeout: Duration,
    reporter: Reporter,
) -> impl Stream<Item = Result<ChatStreamChunk, Error>> {
    async_stream::stream! {
        let mut bytes = response.bytes_stream();
        let mut decoder = EventDecoder::new(reporter);

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
