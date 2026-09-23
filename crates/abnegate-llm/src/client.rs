use std::collections::HashMap;
use std::fmt;
use std::sync::LazyLock;
use std::sync::Mutex;
use std::time::Duration;

use abnegate_secret::REDACTED;
use abnegate_secret::redact;
use reqwest::Client;
use reqwest::Url;
use tokio::runtime;

use crate::error::LlmError;
use crate::reasoning::Effort;
use crate::wire::ChatRequest;
use crate::wire::ChatResponse;
use crate::wire::ChatStreamChunk;
use crate::wire::Message;
use crate::wire::ToolDefinition;

const POOL_IDLE_TIMEOUT: Duration = Duration::from_secs(90);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const POOL_MAX_IDLE_PER_HOST: usize = 16;

fn pool() -> Client {
    Client::builder()
        .pool_max_idle_per_host(POOL_MAX_IDLE_PER_HOST)
        .pool_idle_timeout(POOL_IDLE_TIMEOUT)
        .connect_timeout(CONNECT_TIMEOUT)
        .tcp_nodelay(true)
        .build()
        .unwrap_or_else(|_| Client::new())
}

/// One connection pool per runtime. Building a `reqwest::Client` per turn
/// throws away TLS sessions and keep-alives to the endpoint, which is the whole
/// time-to-first-token budget on a local model, so completions share one.
///
/// They cannot share more widely than the runtime. Every pooled connection is
/// driven by a task belonging to the runtime that opened it, so a pool reused
/// from a second runtime hands out connections whose driver died with the
/// first, and the send fails with "runtime dropped the dispatch task" without
/// ever reaching the server.
static POOLS: LazyLock<Mutex<HashMap<runtime::Id, Client>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn client() -> Client {
    let Ok(runtime) = runtime::Handle::try_current() else {
        return pool();
    };
    let mut pools = POOLS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    pools.entry(runtime.id()).or_insert_with(pool).clone()
}

/// How an [`LlmClient`] reaches its endpoint.
#[derive(Clone)]
pub struct LlmConfig {
    /// Base URL for the API, such as `https://api.openai.com/v1`.
    pub base_url: String,
    pub api_key: String,
    pub default_model: String,
    pub temperature: f32,
    pub max_tokens: u32,
}

/// The key is the one field here that must never be printed, and this config
/// is embedded in the client that every caller logs.
impl fmt::Debug for LlmConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LlmConfig")
            .field("base_url", &self.base_url)
            .field("api_key", &REDACTED)
            .field("default_model", &self.default_model)
            .field("temperature", &self.temperature)
            .field("max_tokens", &self.max_tokens)
            .finish()
    }
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            base_url: "https://api.openai.com/v1".to_string(),
            api_key: String::new(),
            default_model: "gpt-4".to_string(),
            temperature: 0.7,
            max_tokens: 4096,
        }
    }
}

/// Per-request output reservation. Must match the context budget calculation.
#[derive(Debug, Clone, Copy)]
pub struct RequestOptions {
    pub reserved: u32,
}

/// A client for an OpenAI-compatible chat completions endpoint.
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
            client: client(),
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

    fn body(&self, request: ChatRequest<'_>) -> Result<serde_json::Value, LlmError> {
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

    fn validate(&self, url: &str) -> Result<(), LlmError> {
        let parsed = Url::parse(url).map_err(|_| {
            LlmError::InvalidConfig("LLM base_url must be a valid absolute URL".to_string())
        })?;

        match parsed.scheme() {
            "http" | "https" => {}
            _ => {
                return Err(LlmError::InvalidConfig(
                    "LLM base_url must use http or https".to_string(),
                ));
            }
        }

        if !parsed.username().is_empty() || parsed.password().is_some() {
            return Err(LlmError::InvalidConfig(
                "LLM base_url must not include userinfo".to_string(),
            ));
        }

        if parsed.host_str().is_none() {
            return Err(LlmError::InvalidConfig(
                "LLM base_url must include a host".to_string(),
            ));
        }

        // No private-network rule here on purpose. Every caller passes an
        // operator-configured host, and a self-hosted deployment points at
        // loopback, a LAN address or a compose service name. The check belongs
        // where a tenant-supplied host is first accepted, against that value.
        Ok(())
    }

    async fn send(
        &self,
        url: &str,
        body: &serde_json::Value,
    ) -> Result<reqwest::Response, LlmError> {
        self.validate(url)?;
        Ok(self
            .client
            .post(url)
            .header("Authorization", format!("Bearer {}", self.config.api_key))
            .header("Content-Type", "application/json")
            .json(body)
            .send()
            .await?)
    }

    async fn dispatch(&self, request: ChatRequest<'_>) -> Result<reqwest::Response, LlmError> {
        let url = format!("{}/chat/completions", self.config.base_url);
        let response = self.send(&url, &self.body(request)?).await?;

        let status = response.status();
        if status.is_success() {
            return Ok(response);
        }

        let body = response.text().await.unwrap_or_default();
        Err(LlmError::Api {
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
            max_tokens: Some(options.reserved),
            stream: Some(stream),
            stop: (!self.stop.is_empty()).then_some(self.stop.as_slice()),
        }
    }

    fn reserved(&self) -> RequestOptions {
        RequestOptions {
            reserved: self.config.max_tokens,
        }
    }

    /// Make a chat completion request against the configured model.
    pub async fn chat(
        &self,
        messages: &[Message],
        tools: Option<&[ToolDefinition]>,
    ) -> Result<ChatResponse, LlmError> {
        self.chat_with_model(&self.config.default_model, messages, tools)
            .await
    }

    /// Make a chat completion request against a specific model.
    pub async fn chat_with_model(
        &self,
        model: &str,
        messages: &[Message],
        tools: Option<&[ToolDefinition]>,
    ) -> Result<ChatResponse, LlmError> {
        self.chat_with_options(model, messages, tools, self.reserved())
            .await
    }

    pub async fn chat_with_options(
        &self,
        model: &str,
        messages: &[Message],
        tools: Option<&[ToolDefinition]>,
        options: RequestOptions,
    ) -> Result<ChatResponse, LlmError> {
        let response = self
            .dispatch(self.request(model, messages, tools, options, false))
            .await?;
        Ok(response.json().await?)
    }

    /// Stream a chat completion from the configured model.
    pub async fn chat_stream(
        &self,
        messages: &[Message],
        tools: Option<&[ToolDefinition]>,
    ) -> Result<impl futures::Stream<Item = Result<ChatStreamChunk, LlmError>> + use<>, LlmError>
    {
        self.chat_stream_with_model(&self.config.default_model, messages, tools)
            .await
    }

    /// Stream a chat completion from a specific model.
    pub async fn chat_stream_with_model(
        &self,
        model: &str,
        messages: &[Message],
        tools: Option<&[ToolDefinition]>,
    ) -> Result<impl futures::Stream<Item = Result<ChatStreamChunk, LlmError>> + use<>, LlmError>
    {
        self.chat_stream_with_options(model, messages, tools, self.reserved())
            .await
    }

    pub async fn chat_stream_with_options(
        &self,
        model: &str,
        messages: &[Message],
        tools: Option<&[ToolDefinition]>,
        options: RequestOptions,
    ) -> Result<impl futures::Stream<Item = Result<ChatStreamChunk, LlmError>> + use<>, LlmError>
    {
        let response = self
            .dispatch(self.request(model, messages, tools, options, true))
            .await?;

        // Parse SSE without reallocating the leftover buffer on every line.
        Ok(async_stream::stream! {
            let mut bytes = response.bytes_stream();
            let mut buffer = Vec::<u8>::new();

            use futures::StreamExt;
            while let Some(chunk) = bytes.next().await {
                let chunk = match chunk {
                    Ok(chunk) => chunk,
                    Err(error) => {
                        yield Err(LlmError::from(error));
                        break;
                    }
                };

                buffer.extend_from_slice(&chunk);
                let mut consumed = 0_usize;
                let mut done = false;
                while let Some(offset) = buffer[consumed..].iter().position(|byte| *byte == b'\n') {
                    let end = consumed + offset;
                    let mut line = &buffer[consumed..end];
                    if let Some(without_carriage_return) = line.strip_suffix(b"\r") {
                        line = without_carriage_return;
                    }
                    consumed = end + 1;
                    if line.is_empty() {
                        continue;
                    }
                    if let Some(data) = line.strip_prefix(b"data: ") {
                        if data == b"[DONE]" {
                            done = true;
                            break;
                        }
                        match serde_json::from_slice::<ChatStreamChunk>(data) {
                            Ok(chunk) => yield Ok(chunk),
                            Err(error) => yield Err(LlmError::Json(error)),
                        }
                    }
                }
                if consumed > 0 {
                    buffer.drain(..consumed);
                }
                if done {
                    break;
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::LlmClient;
    use super::LlmConfig;
    use crate::error::LlmError;
    use crate::reasoning::Effort;
    use crate::wire::ChatRequest;
    use crate::wire::Message;

    fn client_for(base_url: &str) -> LlmClient {
        LlmClient::new(LlmConfig {
            base_url: base_url.to_string(),
            ..LlmConfig::default()
        })
    }

    #[test]
    fn a_default_config_points_at_openai() {
        let config = LlmConfig::default();

        assert_eq!(config.base_url, "https://api.openai.com/v1");
        assert_eq!(config.api_key, "");
        assert_eq!(config.default_model, "gpt-4");
        assert!((config.temperature - 0.7).abs() < f32::EPSILON);
        assert_eq!(config.max_tokens, 4096);
    }

    #[test]
    fn a_custom_config_keeps_every_field_it_was_given() {
        let config = LlmConfig {
            base_url: "https://custom.api.com/v1".to_string(),
            api_key: "sk-test-key-123".to_string(),
            default_model: "gpt-3.5-turbo".to_string(),
            temperature: 0.5,
            max_tokens: 2048,
        };

        assert_eq!(config.base_url, "https://custom.api.com/v1");
        assert_eq!(config.api_key, "sk-test-key-123");
        assert_eq!(config.default_model, "gpt-3.5-turbo");
        assert!((config.temperature - 0.5).abs() < f32::EPSILON);
        assert_eq!(config.max_tokens, 2048);
    }

    #[test]
    fn a_cloned_config_matches_its_original() {
        let config = LlmConfig {
            base_url: "https://test.api.com".to_string(),
            api_key: "test-key".to_string(),
            default_model: "test-model".to_string(),
            temperature: 0.9,
            max_tokens: 1000,
        };

        let cloned = config.clone();
        assert_eq!(cloned.base_url, config.base_url);
        assert_eq!(cloned.api_key, config.api_key);
        assert_eq!(cloned.default_model, config.default_model);
        assert!((cloned.temperature - config.temperature).abs() < f32::EPSILON);
        assert_eq!(cloned.max_tokens, config.max_tokens);
    }

    /// A default config's `api_key` is the empty string, so asserting that its
    /// debug output merely *mentions* `api_key` is satisfied by a derived
    /// `Debug` -- the exact defect the hand-written one exists to prevent. The
    /// key here is a real one, and what is asserted is that its value is absent.
    #[test]
    fn a_config_never_prints_its_key() {
        const KEY: &str = "sk-test-3f8a1c9e04b27d65";
        let config = LlmConfig {
            api_key: KEY.to_string(),
            ..LlmConfig::default()
        };
        let debug = format!("{config:?}");

        assert!(debug.contains("LlmConfig"));
        assert!(debug.contains("base_url"));
        assert!(debug.contains("default_model"));
        assert!(
            debug.contains("api_key"),
            "the field should still be named, so its redaction is visible: {debug}"
        );
        assert!(
            !debug.contains(KEY),
            "the provider key reached a debug line: {debug}"
        );
    }

    #[test]
    fn a_client_exposes_the_config_it_was_built_from() {
        let config = LlmConfig::default();
        let client = LlmClient::new(config.clone());

        assert_eq!(client.config().base_url, config.base_url);
        assert_eq!(client.config().api_key, config.api_key);
        assert_eq!(client.config().default_model, config.default_model);
    }

    #[test]
    fn a_client_keeps_every_custom_setting() {
        let client = LlmClient::new(LlmConfig {
            base_url: "https://custom.openai.com/v1".to_string(),
            api_key: "sk-custom-key".to_string(),
            default_model: "gpt-4-turbo".to_string(),
            temperature: 0.3,
            max_tokens: 8192,
        });

        assert_eq!(client.config().base_url, "https://custom.openai.com/v1");
        assert_eq!(client.config().api_key, "sk-custom-key");
        assert_eq!(client.config().default_model, "gpt-4-turbo");
        assert!((client.config().temperature - 0.3).abs() < f32::EPSILON);
        assert_eq!(client.config().max_tokens, 8192);
    }

    #[test]
    fn a_cloned_client_shares_its_config() {
        let client = LlmClient::new(LlmConfig {
            base_url: "https://test.api.com".to_string(),
            api_key: "test-key".to_string(),
            default_model: "test-model".to_string(),
            temperature: 0.6,
            max_tokens: 512,
        });
        let cloned = client.clone();

        assert_eq!(cloned.config().base_url, client.config().base_url);
        assert_eq!(cloned.config().api_key, client.config().api_key);
    }

    #[test]
    fn a_client_never_prints_its_key() {
        const KEY: &str = "sk-test-6b02da97e15c4f38";
        let client = LlmClient::new(LlmConfig {
            api_key: KEY.to_string(),
            ..LlmConfig::default()
        });
        let debug = format!("{client:?}");

        assert!(debug.contains("LlmClient"));
        assert!(debug.contains("config"));
        assert!(
            !debug.contains(KEY),
            "the provider key reached a debug line through the client: {debug}"
        );
    }

    #[test]
    fn a_temperature_override_replaces_only_the_temperature() {
        let client = LlmClient::new(LlmConfig::default()).with_temperature(0.1);

        assert!((client.config().temperature - 0.1).abs() < f32::EPSILON);
        assert_eq!(client.config().default_model, "gpt-4");
    }

    #[test]
    fn stop_strings_reach_the_body_only_once_set() {
        let messages = [Message::user("Hi")];
        let plain = LlmClient::new(LlmConfig::default());
        let body = plain
            .body(plain.request("gpt-4", &messages, None, plain.reserved(), false))
            .unwrap();
        assert!(body.get("stop").is_none());

        let halting = plain.with_stop(vec!["<|end|>".to_string()]);
        let body = halting
            .body(halting.request("gpt-4", &messages, None, halting.reserved(), false))
            .unwrap();
        assert_eq!(body["stop"][0], "<|end|>");
    }

    #[test]
    fn an_ollama_context_limit_is_model_bound() {
        let messages = [Message::user("Hi")];
        let client = LlmClient::new(LlmConfig::default()).with_ollama_context("local", 32_768);

        let matched = client
            .body(client.request("local", &messages, None, client.reserved(), false))
            .unwrap();
        assert_eq!(matched["num_ctx"], 32_768);

        let other = client
            .body(client.request("other", &messages, None, client.reserved(), false))
            .unwrap();
        assert!(other.get("num_ctx").is_none());
    }

    #[test]
    fn a_streaming_request_asks_for_usage() {
        let messages = [Message::user("Hi")];
        let client = LlmClient::new(LlmConfig::default());

        let body = client
            .body(client.request("gpt-4", &messages, None, client.reserved(), true))
            .unwrap();
        assert_eq!(body["stream_options"]["include_usage"], true);
    }

    #[test]
    fn reasoning_effort_is_model_bound_and_uses_the_resolved_level() {
        let messages = [Message::user("Hi")];
        let client = LlmClient::new(LlmConfig::default()).with_reasoning("thinker", Effort::High);
        let enabled = client
            .body(ChatRequest {
                model: "thinker",
                messages: &messages,
                tools: None,
                tool_choice: None,
                temperature: None,
                max_tokens: Some(4096),
                stream: None,
                stop: None,
            })
            .unwrap();
        assert_eq!(enabled["reasoning_effort"], "high");
        let other = client
            .body(ChatRequest {
                model: "other",
                messages: &messages,
                tools: None,
                tool_choice: None,
                temperature: None,
                max_tokens: Some(4096),
                stream: None,
                stop: None,
            })
            .unwrap();
        assert!(other.get("reasoning_effort").is_none());
        let compact = client
            .without_reasoning()
            .body(ChatRequest {
                model: "thinker",
                messages: &messages,
                tools: None,
                tool_choice: None,
                temperature: None,
                max_tokens: Some(4096),
                stream: None,
                stop: None,
            })
            .unwrap();
        assert!(compact.get("reasoning_effort").is_none());
    }

    #[tokio::test]
    async fn a_rejection_body_echoing_the_key_is_redacted() {
        const ECHOED: &str = "sk-proj-0b1c2d3e4f5a6b7c8d9e0f1a2b3c4d5e";
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .respond_with(
                wiremock::ResponseTemplate::new(401).set_body_string(format!(
                    r#"{{"error":{{"message":"Incorrect API key provided: {ECHOED}"}}}}"#
                )),
            )
            .mount(&server)
            .await;

        let error = client_for(&server.uri())
            .chat(&[Message::user("hello")], None)
            .await
            .expect_err("a 401 is a failure");

        let rendered = format!("{error} {error:?}");
        assert!(!rendered.contains(ECHOED), "the key leaked: {rendered}");
        assert!(matches!(error, LlmError::Api { status: 401, .. }));
    }

    #[tokio::test]
    async fn a_transport_failure_never_carries_the_url() {
        const QUERY_SECRET: &str = "a9f3e1c7b5d2046e8f1a3c5e7b9d0f2a";
        let error = client_for(&format!("http://127.0.0.1:1/v1?key={QUERY_SECRET}"))
            .chat(&[Message::user("hello")], None)
            .await
            .expect_err("nothing listens on port 1");

        let rendered = format!("{error} {error:?}");
        assert!(matches!(error, LlmError::Http(_)), "{rendered}");
        assert!(
            !rendered.contains(QUERY_SECRET),
            "the URL reached the error: {rendered}"
        );
    }

    #[test]
    fn a_public_https_host_is_accepted() {
        assert!(
            client_for("https://api.openai.com/v1")
                .validate("https://api.openai.com/v1/chat/completions")
                .is_ok()
        );
    }

    #[test]
    fn a_non_http_scheme_is_refused() {
        let error = client_for("file:///etc/passwd")
            .validate("file:///etc/passwd")
            .unwrap_err();
        assert!(
            matches!(&error, LlmError::InvalidConfig(message) if message.contains("http or https")),
            "the refusal has to name the scheme rule: {error}"
        );
    }

    #[test]
    fn credentials_in_the_url_are_refused() {
        let error = client_for("https://user:pass@api.openai.com/v1")
            .validate("https://user:pass@api.openai.com/v1")
            .unwrap_err();
        assert!(
            matches!(&error, LlmError::InvalidConfig(message) if message.contains("userinfo")),
            "the refusal has to name the userinfo rule: {error}"
        );
    }

    #[test]
    fn the_hosts_a_self_hosted_deployment_runs_on_are_accepted() {
        for base_url in [
            "http://localhost:4000",
            "http://127.0.0.1:11434",
            "http://192.168.1.50:4000",
            "http://host.docker.internal:11434",
            "http://gateway:4000",
            "http://[::1]:4000",
        ] {
            assert!(
                client_for(base_url).validate(base_url).is_ok(),
                "{base_url} is a supported way to reach a self-hosted model server"
            );
        }
    }

    #[test]
    fn a_relative_url_is_refused() {
        let error = client_for("/v1/chat/completions")
            .validate("/v1/chat/completions")
            .unwrap_err();
        assert!(
            matches!(&error, LlmError::InvalidConfig(message) if message.contains("absolute URL")),
            "the refusal has to name the absolute-URL rule: {error}"
        );
    }
}
