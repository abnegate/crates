use std::process::Stdio;
use std::time::Duration;

use abnegate_secret::SecretValue;
use async_trait::async_trait;
use futures::Stream;

use crate::modality::{ResponseFormat, TextProvider, TextRequest, TextResponse};
use crate::provider::ProviderError;

const BASE_URL: &str = "https://api.anthropic.com";
const CLI: &str = "claude";
const DEFAULT_MODEL: &str = "claude-opus-5";
const MAX_CONTEXT_TOKENS: u32 = 200_000;
const TIMEOUT: Duration = Duration::from_secs(6 * 60 * 60);
const TOOL: &str = "structured_output";
const VERSION: &str = "2023-06-01";

/// How a call proves who it is.
pub enum AnthropicAuth {
    ApiKey(SecretValue),
    OAuthToken(SecretValue),
    /// No credentials of our own: the installed `claude` CLI holds them.
    ClaudeCli,
}

pub struct AnthropicProvider {
    auth: AnthropicAuth,
    model: String,
    base_url: String,
    client: reqwest::Client,
}

impl AnthropicProvider {
    pub fn new(api_key: impl Into<SecretValue>) -> Self {
        Self::with_model(AnthropicAuth::ApiKey(api_key.into()), DEFAULT_MODEL)
    }

    pub fn with_oauth(token: impl Into<SecretValue>) -> Self {
        Self::with_model(AnthropicAuth::OAuthToken(token.into()), DEFAULT_MODEL)
    }

    pub fn with_model(auth: AnthropicAuth, model: &str) -> Self {
        Self::with_base_url(auth, model, BASE_URL)
    }

    /// A provider that talks to `base_url` instead of the public API, for a
    /// proxy, a gateway or a test double.
    pub fn with_base_url(auth: AnthropicAuth, model: &str, base_url: impl Into<String>) -> Self {
        Self {
            auth,
            model: model.to_string(),
            base_url: base_url.into(),
            client: reqwest::Client::builder()
                .timeout(TIMEOUT)
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
        }
    }

    pub fn build_request_body(&self, request: &TextRequest) -> serde_json::Value {
        serde_json::json!({
            "model": self.model,
            "max_tokens": request.max_tokens,
            "system": request.system_prompt,
            "messages": [
                { "role": "user", "content": request.user_prompt }
            ],
            "temperature": request.temperature
        })
    }

    pub fn parse_response(body: &serde_json::Value) -> Result<TextResponse, ProviderError> {
        let content = body
            .get("content")
            .and_then(|content| content.as_array())
            .and_then(|content| content.first())
            .and_then(|block| block.get("text"))
            .and_then(|text| text.as_str())
            .ok_or_else(|| ProviderError::parse("missing content[0].text in response"))?
            .to_string();

        Ok(TextResponse {
            content,
            model: body
                .get("model")
                .and_then(|model| model.as_str())
                .unwrap_or("unknown")
                .to_string(),
            input_tokens: usage(body, "input_tokens"),
            output_tokens: usage(body, "output_tokens"),
            finish_reason: body
                .get("stop_reason")
                .and_then(|reason| reason.as_str())
                .unwrap_or("end_turn")
                .to_string(),
        })
    }

    /// Whether this provider must go through the CLI rather than HTTP.
    ///
    /// An OAuth token is the CLI's own credential and is not accepted by the
    /// HTTP API, so both routes lead to the same place.
    pub fn uses_cli(&self) -> bool {
        matches!(
            self.auth,
            AnthropicAuth::OAuthToken(_) | AnthropicAuth::ClaudeCli
        )
    }

    fn authenticate(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.auth {
            AnthropicAuth::ApiKey(key) => request.header("x-api-key", key.expose()),
            AnthropicAuth::OAuthToken(token) => {
                request.header("Authorization", format!("Bearer {}", token.expose()))
            }
            AnthropicAuth::ClaudeCli => request,
        }
    }

    async fn send(&self, body: &serde_json::Value) -> Result<serde_json::Value, ProviderError> {
        let request = self
            .client
            .post(format!("{}/v1/messages", self.base_url))
            .header("anthropic-version", VERSION)
            .header("content-type", "application/json")
            .json(body);

        let response = self
            .authenticate(request)
            .send()
            .await
            .map_err(|error| ProviderError::network(error.to_string()))?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let message = response
                .text()
                .await
                .unwrap_or_else(|_| "unknown error".into());
            return Err(ProviderError::api(status, message));
        }

        response
            .json()
            .await
            .map_err(|error| ProviderError::parse(error.to_string()))
    }

    /// Ask the CLI for an answer that satisfies `schema`.
    ///
    /// `--json-schema` makes the CLI enforce the shape, so a caller gets
    /// either a conforming value or an error, never prose it has to guess at.
    async fn structured_via_cli(
        &self,
        request: &TextRequest,
        schema: &serde_json::Value,
    ) -> Result<serde_json::Value, ProviderError> {
        let prompt = joined_prompt(request, "\n\n");
        let schema = serde_json::to_string(schema)
            .map_err(|error| ProviderError::config(format!("unserialisable schema: {error}")))?;

        let output = tokio::process::Command::new(CLI)
            .arg("--print")
            .arg("--output-format")
            .arg("json")
            .arg("--model")
            .arg(&self.model)
            .arg("--json-schema")
            .arg(&schema)
            .arg("-p")
            .arg(&prompt)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .map_err(|error| {
                ProviderError::network(format!(
                    "could not run the {CLI} CLI: {error}. Install it, or choose another provider."
                ))
            })?;

        if !output.status.success() {
            return Err(cli_failed(&output));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let envelope: serde_json::Value = serde_json::from_str(&stdout).map_err(|error| {
            ProviderError::parse(format!(
                "the {CLI} CLI did not return JSON: {error}. Raw: {}",
                truncated(&stdout, 500)
            ))
        })?;

        Self::unwrap_cli_result(envelope)
    }

    /// Take the answer out of the CLI's envelope.
    ///
    /// With `--json-schema` the result is the value itself, under
    /// `structured_output`; without one it is a string under `result` that may
    /// hold JSON. Both shapes appear in practice.
    fn unwrap_cli_result(envelope: serde_json::Value) -> Result<serde_json::Value, ProviderError> {
        let inner = envelope
            .get(TOOL)
            .or_else(|| envelope.get("result"))
            .cloned()
            .unwrap_or(envelope);

        match inner {
            serde_json::Value::Null => Err(ProviderError::parse(format!(
                "the {CLI} CLI returned no result"
            ))),
            serde_json::Value::String(text) => serde_json::from_str(&text).map_err(|error| {
                ProviderError::parse(format!(
                    "the model's answer was not the JSON the schema asked for: {error}. \
                     Answer: {}",
                    truncated(&text, 300)
                ))
            }),
            value => Ok(value),
        }
    }

    async fn complete_via_cli(&self, request: &TextRequest) -> Result<TextResponse, ProviderError> {
        let prompt = if request.system_prompt.is_empty() {
            request.user_prompt.clone()
        } else {
            format!(
                "<system>\n{}\n</system>\n\n{}",
                request.system_prompt, request.user_prompt
            )
        };

        let output = tokio::process::Command::new(CLI)
            .arg("-p")
            .arg(&prompt)
            .arg(format!("--model={}", self.model))
            .arg("--output-format=json")
            .arg("--max-turns=1")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .map_err(|error| {
                ProviderError::network(format!(
                    "could not run the {CLI} CLI: {error}. Install it, or choose another provider."
                ))
            })?;

        if !output.status.success() {
            return Err(cli_failed(&output));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let envelope: serde_json::Value = serde_json::from_str(&stdout).map_err(|error| {
            ProviderError::parse(format!(
                "the {CLI} CLI did not return JSON: {error}. Raw: {}",
                truncated(&stdout, 500)
            ))
        })?;

        let content = envelope
            .get("result")
            .or_else(|| envelope.get("content"))
            .and_then(|content| content.as_str())
            .unwrap_or_default()
            .to_string();

        if content.is_empty() {
            return Err(ProviderError::parse(format!(
                "the {CLI} CLI returned an empty answer. Raw: {}",
                truncated(&stdout, 1000)
            )));
        }

        Ok(TextResponse {
            content,
            model: self.model.clone(),
            input_tokens: 0,
            output_tokens: 0,
            finish_reason: "end_turn".into(),
        })
    }
}

fn joined_prompt(request: &TextRequest, separator: &str) -> String {
    if request.system_prompt.is_empty() {
        return request.user_prompt.clone();
    }
    format!(
        "{}{separator}{}",
        request.system_prompt, request.user_prompt
    )
}

fn cli_failed(output: &std::process::Output) -> ProviderError {
    let stderr = String::from_utf8_lossy(&output.stderr);
    ProviderError::network(format!(
        "the {CLI} CLI exited with {}: {}",
        output.status,
        truncated(&stderr, 500)
    ))
}

fn truncated(text: &str, characters: usize) -> String {
    text.chars().take(characters).collect()
}

fn usage(body: &serde_json::Value, field: &str) -> u32 {
    body.get("usage")
        .and_then(|usage| usage.get(field))
        .and_then(|count| count.as_u64())
        .unwrap_or(0) as u32
}

#[async_trait]
impl TextProvider for AnthropicProvider {
    fn name(&self) -> &str {
        "anthropic"
    }

    fn supports_structured_output(&self) -> bool {
        true
    }

    fn max_context_tokens(&self) -> u32 {
        MAX_CONTEXT_TOKENS
    }

    async fn complete(&self, request: &TextRequest) -> Result<TextResponse, ProviderError> {
        if self.uses_cli() {
            return self.complete_via_cli(request).await;
        }

        let body = self.build_request_body(request);
        Self::parse_response(&self.send(&body).await?)
    }

    async fn complete_structured(
        &self,
        request: &TextRequest,
    ) -> Result<serde_json::Value, ProviderError> {
        let Some(ResponseFormat::Json {
            schema: Some(schema),
        }) = &request.response_format
        else {
            let response = self.complete(request).await?;
            return serde_json::from_str(&response.content).map_err(|error| {
                ProviderError::parse(format!("failed to parse structured output: {error}"))
            });
        };

        // The HTTP body below needs an API key, which a CLI-only provider has
        // none of: without this it would fall through and fail.
        if self.uses_cli() {
            return self.structured_via_cli(request, schema).await;
        }

        let body = serde_json::json!({
            "model": self.model,
            "max_tokens": request.max_tokens,
            "system": request.system_prompt,
            "messages": [
                { "role": "user", "content": request.user_prompt }
            ],
            "temperature": request.temperature,
            "tools": [{
                "name": TOOL,
                "description": "Return your response in this exact structured format",
                "input_schema": schema
            }],
            "tool_choice": { "type": "tool", "name": TOOL }
        });

        let json = self.send(&body).await?;

        json.get("content")
            .and_then(|content| content.as_array())
            .and_then(|blocks| {
                blocks.iter().find(|block| {
                    block.get("type").and_then(|kind| kind.as_str()) == Some("tool_use")
                })
            })
            .and_then(|block| block.get("input"))
            .cloned()
            .ok_or_else(|| {
                ProviderError::parse("missing tool_use content block in structured response")
            })
    }

    async fn stream_complete(
        &self,
        _request: &TextRequest,
    ) -> Result<Box<dyn Stream<Item = Result<String, ProviderError>> + Send + Unpin>, ProviderError>
    {
        Err(ProviderError::unsupported(
            "streaming not yet implemented for Anthropic provider",
        ))
    }
}

#[cfg(test)]
mod tests {
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    fn provider() -> AnthropicProvider {
        AnthropicProvider::new("test-key")
    }

    #[test]
    fn a_body_carries_the_model_the_system_prompt_and_the_message() {
        let request = TextRequest {
            system_prompt: "You are a helpful assistant.".into(),
            user_prompt: "Hello, world!".into(),
            temperature: 0.7,
            max_tokens: 1024,
            response_format: None,
            context: None,
        };

        let body = provider().build_request_body(&request);

        assert_eq!(body["model"], DEFAULT_MODEL);
        assert_eq!(body["max_tokens"], 1024);
        assert_eq!(body["system"], "You are a helpful assistant.");
        assert_eq!(body["temperature"], 0.7);

        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["role"], "user");
        assert_eq!(messages[0]["content"], "Hello, world!");
    }

    #[test]
    fn a_custom_model_reaches_the_body() {
        let provider = AnthropicProvider::with_model(
            AnthropicAuth::ApiKey(SecretValue::new("key")),
            "claude-sonnet-5",
        );
        let body = provider.build_request_body(&TextRequest::new("sys", "usr"));
        assert_eq!(body["model"], "claude-sonnet-5");
    }

    #[test]
    fn the_extremes_of_a_request_survive_the_body() {
        let mut request = TextRequest::new("precise", "classify this");
        request.temperature = 0.0;
        request.max_tokens = 200_000;

        let body = provider().build_request_body(&request);

        assert_eq!(body["temperature"], 0.0);
        assert_eq!(body["max_tokens"], 200_000);
    }

    #[test]
    fn a_response_is_read_in_full() {
        let body = serde_json::json!({
            "id": "msg_123",
            "type": "message",
            "role": "assistant",
            "model": "claude-opus-5",
            "content": [{ "type": "text", "text": "Hello! How can I help you today?" }],
            "stop_reason": "end_turn",
            "usage": { "input_tokens": 25, "output_tokens": 15 }
        });

        let response = AnthropicProvider::parse_response(&body).unwrap();

        assert_eq!(response.content, "Hello! How can I help you today?");
        assert_eq!(response.model, "claude-opus-5");
        assert_eq!(response.input_tokens, 25);
        assert_eq!(response.output_tokens, 15);
        assert_eq!(response.finish_reason, "end_turn");
    }

    #[test]
    fn a_response_without_content_is_an_error_not_an_empty_answer() {
        let error =
            AnthropicProvider::parse_response(&serde_json::json!({ "id": "msg_123" })).unwrap_err();
        match error {
            ProviderError::Parse { detail } => assert!(detail.contains("content"), "{detail}"),
            other => panic!("expected a parse failure, got {other:?}"),
        }
    }

    #[test]
    fn the_optional_parts_of_a_response_have_defaults() {
        let body = serde_json::json!({ "content": [{ "type": "text", "text": "Hi" }] });

        let response = AnthropicProvider::parse_response(&body).unwrap();

        assert_eq!(response.model, "unknown");
        assert_eq!(response.finish_reason, "end_turn");
        assert_eq!(response.input_tokens, 0);
        assert_eq!(response.output_tokens, 0);
    }

    #[test]
    fn the_provider_reports_what_it_can_do() {
        let provider = provider();
        assert_eq!(provider.name(), "anthropic");
        assert!(provider.supports_structured_output());
        assert_eq!(provider.max_context_tokens(), MAX_CONTEXT_TOKENS);
    }

    #[test]
    fn an_oauth_token_and_the_bare_cli_both_go_through_the_cli() {
        assert!(AnthropicProvider::with_oauth("oauth-token-123").uses_cli());
        assert!(AnthropicProvider::with_model(AnthropicAuth::ClaudeCli, DEFAULT_MODEL).uses_cli());
        assert!(!provider().uses_cli());
    }

    #[test]
    fn a_schema_enforced_answer_is_taken_out_of_the_cli_envelope() {
        let envelope = serde_json::json!({ "structured_output": { "beats": 3 } });
        let value = AnthropicProvider::unwrap_cli_result(envelope).unwrap();
        assert_eq!(value["beats"], 3);
    }

    #[test]
    fn a_json_string_answer_is_parsed_out_of_the_cli_envelope() {
        let envelope = serde_json::json!({ "result": r#"{"beats": 3}"# });
        let value = AnthropicProvider::unwrap_cli_result(envelope).unwrap();
        assert_eq!(value["beats"], 3);
    }

    #[test]
    fn a_non_json_answer_from_the_cli_is_refused() {
        let envelope = serde_json::json!({ "result": "not json at all" });
        let error = AnthropicProvider::unwrap_cli_result(envelope).unwrap_err();
        assert!(matches!(error, ProviderError::Parse { .. }), "{error:?}");
    }

    #[test]
    fn an_empty_cli_envelope_is_refused() {
        let envelope = serde_json::json!({ "result": serde_json::Value::Null });
        let error = AnthropicProvider::unwrap_cli_result(envelope).unwrap_err();
        assert!(matches!(error, ProviderError::Parse { .. }), "{error:?}");
    }

    #[tokio::test]
    async fn a_completion_is_sent_with_the_api_key_and_read_back() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .and(header("x-api-key", "sk-test"))
            .and(header("anthropic-version", VERSION))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "model": DEFAULT_MODEL,
                "content": [{ "type": "text", "text": "hello" }],
                "stop_reason": "end_turn",
                "usage": { "input_tokens": 4, "output_tokens": 2 }
            })))
            .mount(&server)
            .await;

        let provider = AnthropicProvider::with_base_url(
            AnthropicAuth::ApiKey(SecretValue::new("sk-test")),
            DEFAULT_MODEL,
            server.uri(),
        );
        let response = provider
            .complete(&TextRequest::new("sys", "usr"))
            .await
            .unwrap();

        assert_eq!(response.content, "hello");
        assert_eq!(response.input_tokens, 4);
    }

    #[tokio::test]
    async fn a_structured_answer_comes_from_the_tool_use_block() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "content": [
                    { "type": "text", "text": "thinking" },
                    { "type": "tool_use", "name": TOOL, "input": { "title": "Requiem" } }
                ]
            })))
            .mount(&server)
            .await;

        let provider = AnthropicProvider::with_base_url(
            AnthropicAuth::ApiKey(SecretValue::new("sk-test")),
            DEFAULT_MODEL,
            server.uri(),
        );
        let mut request = TextRequest::new("sys", "usr");
        request.response_format = Some(ResponseFormat::Json {
            schema: Some(serde_json::json!({ "title": "Facts" })),
        });

        let value = provider.complete_structured(&request).await.unwrap();

        assert_eq!(value["title"], "Requiem");
    }

    #[tokio::test]
    async fn a_structured_answer_with_no_tool_use_block_is_refused() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "content": [{ "type": "text", "text": "prose instead" }]
            })))
            .mount(&server)
            .await;

        let provider = AnthropicProvider::with_base_url(
            AnthropicAuth::ApiKey(SecretValue::new("sk-test")),
            DEFAULT_MODEL,
            server.uri(),
        );
        let mut request = TextRequest::new("sys", "usr");
        request.response_format = Some(ResponseFormat::Json {
            schema: Some(serde_json::json!({ "title": "Facts" })),
        });

        let error = provider.complete_structured(&request).await.unwrap_err();

        assert!(matches!(error, ProviderError::Parse { .. }), "{error:?}");
    }

    #[tokio::test]
    async fn a_refused_call_carries_the_status_and_the_body() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(ResponseTemplate::new(529).set_body_string("overloaded"))
            .mount(&server)
            .await;

        let provider = AnthropicProvider::with_base_url(
            AnthropicAuth::ApiKey(SecretValue::new("sk-test")),
            DEFAULT_MODEL,
            server.uri(),
        );
        let error = provider
            .complete(&TextRequest::new("sys", "usr"))
            .await
            .unwrap_err();

        match error {
            ProviderError::Api { status, message } => {
                assert_eq!(status, 529);
                assert_eq!(message, "overloaded");
            }
            other => panic!("expected ApiError, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn streaming_says_it_is_not_implemented() {
        let error = provider()
            .stream_complete(&TextRequest::new("sys", "usr"))
            .await
            .err()
            .unwrap();
        assert!(
            matches!(error, ProviderError::Unsupported { .. }),
            "{error:?}"
        );
    }
}
