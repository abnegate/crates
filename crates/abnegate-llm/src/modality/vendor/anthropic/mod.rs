//! The Anthropic messages API, or the Claude Code CLI when the credential
//! is the CLI's own.

mod authentication;
mod cli;
mod failure;

use std::path::PathBuf;
use std::time::Duration;

use abnegate_secret::SecretValue;
use async_trait::async_trait;
use futures::Stream;

pub use crate::modality::vendor::anthropic::authentication::AnthropicAuthentication;

use crate::modality::vendor::anthropic::cli::Cli;
use crate::modality::vendor::anthropic::failure::Failure;
use crate::modality::vendor::transport::Transport;
use crate::modality::{
    ResponseFormat, StructuredResponse, TextProvider, TextRequest, TextResponse,
};
use crate::provider::ExitStatus;
use crate::provider::ProviderError;

const BASE_URL: &str = "https://api.anthropic.com";
const DEFAULT_MODEL: &str = "claude-opus-5";
const MAXIMUM_CONTEXT_TOKENS: u32 = 200_000;
const NAME: &str = "anthropic";
const TOOL: &str = "structured_output";
const VERSION: &str = "2023-06-01";
const RAW_PREVIEW_CHARACTERS: usize = 500;
const ANSWER_PREVIEW_CHARACTERS: usize = 300;

/// Claude, over the messages API with an API key, or through the installed
/// `claude` CLI with an OAuth token or the CLI's own sign-in.
///
/// Every call has a deadline (ten minutes unless [`Self::with_timeout`] says
/// otherwise), and the HTTP client never follows a redirect, so the
/// `x-api-key` header cannot be carried to another host.
#[derive(Debug, Clone)]
pub struct AnthropicProvider {
    authentication: AnthropicAuthentication,
    model: String,
    base_url: String,
    transport: Transport,
    cli: Cli,
}

impl AnthropicProvider {
    pub fn new(api_key: impl Into<SecretValue>) -> Self {
        Self::with_model(
            AnthropicAuthentication::ApiKey(api_key.into()),
            DEFAULT_MODEL,
        )
    }

    pub fn with_oauth(token: impl Into<SecretValue>) -> Self {
        Self::with_model(
            AnthropicAuthentication::OAuthToken(token.into()),
            DEFAULT_MODEL,
        )
    }

    pub fn with_model(authentication: AnthropicAuthentication, model: &str) -> Self {
        Self::with_base_url(authentication, model, BASE_URL)
    }

    /// A provider that talks to `base_url` instead of the public API, for a
    /// proxy, a gateway or a test double.
    pub fn with_base_url(
        authentication: AnthropicAuthentication,
        model: &str,
        base_url: impl Into<String>,
    ) -> Self {
        Self {
            authentication,
            model: model.to_string(),
            base_url: base_url.into(),
            transport: Transport::default(),
            cli: Cli::default(),
        }
    }

    /// The deadline for one call, over HTTP or through the CLI.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.transport = Transport::with_timeout(timeout);
        self.cli = self.cli.with_timeout(timeout);
        self
    }

    /// The `claude` executable to run, when it is not the one on `PATH`.
    pub fn with_executable(mut self, executable: impl Into<PathBuf>) -> Self {
        self.cli = self.cli.with_executable(executable);
        self
    }

    pub fn build_request_body(&self, request: &TextRequest) -> serde_json::Value {
        serde_json::json!({
            "model": self.model,
            "max_tokens": request.maximum_tokens,
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
            self.authentication,
            AnthropicAuthentication::OAuthToken(_) | AnthropicAuthentication::ClaudeCli
        )
    }

    fn token(&self) -> Option<&SecretValue> {
        match &self.authentication {
            AnthropicAuthentication::OAuthToken(token) => Some(token),
            AnthropicAuthentication::ApiKey(_) | AnthropicAuthentication::ClaudeCli => None,
        }
    }

    async fn send(&self, body: &serde_json::Value) -> Result<serde_json::Value, ProviderError> {
        let mut request = self
            .transport
            .post(format!("{}/v1/messages", self.base_url))
            .header("anthropic-version", VERSION)
            .json(body);
        if let AnthropicAuthentication::ApiKey(key) = &self.authentication {
            request = request.header("x-api-key", key.expose());
        }
        self.transport.send(request).await
    }

    async fn envelope(
        &self,
        arguments: Vec<String>,
        prompt: &str,
    ) -> Result<(serde_json::Value, String), ProviderError> {
        let output = self.cli.run(NAME, &arguments, prompt, self.token()).await?;
        let stdout = String::from_utf8_lossy(&output).into_owned();
        let envelope = serde_json::from_str(&stdout).map_err(|error| {
            ProviderError::parse(format!(
                "the claude CLI did not return JSON: {error}. Raw: {}",
                truncated(&stdout, RAW_PREVIEW_CHARACTERS)
            ))
        })?;
        if let Some(failure) = Failure::reported(&envelope) {
            return Err(failure.into_error(NAME, ExitStatus::Code(0)));
        }
        Ok((envelope, stdout))
    }

    /// Ask the CLI for an answer that satisfies `schema`.
    ///
    /// `--json-schema` makes the CLI enforce the shape, so a caller gets
    /// either a conforming value or an error, never prose it has to guess at.
    async fn structured_via_cli(
        &self,
        request: &TextRequest,
        schema: &serde_json::Value,
    ) -> Result<StructuredResponse, ProviderError> {
        let schema = serde_json::to_string(schema)
            .map_err(|error| ProviderError::config(format!("unserialisable schema: {error}")))?;
        let arguments = vec![
            "--print".to_string(),
            "--output-format=json".to_string(),
            format!("--model={}", self.model),
            format!("--json-schema={schema}"),
        ];

        let (envelope, _) = self
            .envelope(arguments, &joined_prompt(request, "\n\n"))
            .await?;
        let input_tokens = usage(&envelope, "input_tokens");
        let output_tokens = usage(&envelope, "output_tokens");
        Ok(StructuredResponse {
            value: Self::unwrap_cli_result(envelope)?,
            model: self.model.clone(),
            input_tokens,
            output_tokens,
        })
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
            serde_json::Value::Null => {
                Err(ProviderError::parse("the claude CLI returned no result"))
            }
            serde_json::Value::String(text) => serde_json::from_str(&text).map_err(|error| {
                ProviderError::parse(format!(
                    "the model's answer was not the JSON the schema asked for: {error}. \
                     Answer: {}",
                    truncated(&text, ANSWER_PREVIEW_CHARACTERS)
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
        let arguments = vec![
            "--print".to_string(),
            "--output-format=json".to_string(),
            format!("--model={}", self.model),
            "--max-turns=1".to_string(),
        ];

        let (envelope, stdout) = self.envelope(arguments, &prompt).await?;
        let content = envelope
            .get("result")
            .or_else(|| envelope.get("content"))
            .and_then(|content| content.as_str())
            .unwrap_or_default()
            .to_string();

        if content.is_empty() {
            return Err(ProviderError::parse(format!(
                "the claude CLI returned an empty answer. Raw: {}",
                truncated(&stdout, RAW_PREVIEW_CHARACTERS)
            )));
        }

        Ok(TextResponse {
            content,
            model: self.model.clone(),
            input_tokens: usage(&envelope, "input_tokens"),
            output_tokens: usage(&envelope, "output_tokens"),
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

fn truncated(text: &str, characters: usize) -> String {
    text.chars().take(characters).collect()
}

fn usage(body: &serde_json::Value, field: &str) -> u32 {
    body.get("usage")
        .and_then(|usage| usage.get(field))
        .and_then(|count| count.as_u64())
        .map_or(0, |count| u32::try_from(count).unwrap_or(u32::MAX))
}

#[async_trait]
impl TextProvider for AnthropicProvider {
    fn name(&self) -> &str {
        NAME
    }

    fn supports_structured_output(&self) -> bool {
        true
    }

    fn maximum_context_tokens(&self) -> u32 {
        MAXIMUM_CONTEXT_TOKENS
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
    ) -> Result<StructuredResponse, ProviderError> {
        let Some(ResponseFormat::Json {
            schema: Some(schema),
            ..
        }) = &request.response_format
        else {
            return StructuredResponse::from_text(self.complete(request).await?);
        };

        // The HTTP body below needs an API key, which a CLI-only provider has
        // none of: without this it would fall through and fail.
        if self.uses_cli() {
            return self.structured_via_cli(request, schema).await;
        }

        let body = serde_json::json!({
            "model": self.model,
            "max_tokens": request.maximum_tokens,
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

        let value = json
            .get("content")
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
            })?;

        Ok(StructuredResponse {
            value,
            model: json
                .get("model")
                .and_then(|model| model.as_str())
                .unwrap_or(&self.model)
                .to_string(),
            input_tokens: usage(&json, "input_tokens"),
            output_tokens: usage(&json, "output_tokens"),
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
    use std::time::Instant;

    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    /// A stand-in `claude` that records its token, arguments and stdin next
    /// to itself and answers with `answer`.
    #[cfg(unix)]
    fn fake_cli(directory: &std::path::Path, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;

        let executable = directory.join("claude");
        let record = directory.display();
        std::fs::write(
            &executable,
            format!(
                "#!/bin/sh\nprintf '%s' \"$CLAUDE_CODE_OAUTH_TOKEN\" > '{record}/token'\nprintf '%s\\n' \"$@\" > '{record}/arguments'\ncat > '{record}/stdin'\n{body}\n"
            ),
        )
        .unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
        executable
    }

    fn provider() -> AnthropicProvider {
        AnthropicProvider::new("test-key")
    }

    #[test]
    fn a_body_carries_the_model_the_system_prompt_and_the_message() {
        let request = TextRequest {
            system_prompt: "You are a helpful assistant.".into(),
            user_prompt: "Hello, world!".into(),
            temperature: 0.7,
            maximum_tokens: 1024,
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
            AnthropicAuthentication::ApiKey(SecretValue::new("key")),
            "claude-sonnet-5",
        );
        let body = provider.build_request_body(&TextRequest::new("sys", "usr"));
        assert_eq!(body["model"], "claude-sonnet-5");
    }

    #[test]
    fn the_extremes_of_a_request_survive_the_body() {
        let mut request = TextRequest::new("precise", "classify this");
        request.temperature = 0.0;
        request.maximum_tokens = 200_000;

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
        assert_eq!(provider.maximum_context_tokens(), MAXIMUM_CONTEXT_TOKENS);
    }

    #[test]
    fn an_oauth_token_and_the_bare_cli_both_go_through_the_cli() {
        assert!(AnthropicProvider::with_oauth("oauth-token-123").uses_cli());
        assert!(
            AnthropicProvider::with_model(AnthropicAuthentication::ClaudeCli, DEFAULT_MODEL)
                .uses_cli()
        );
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
            AnthropicAuthentication::ApiKey(SecretValue::new("sk-test")),
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
                ],
                "usage": { "input_tokens": 11, "output_tokens": 5 }
            })))
            .mount(&server)
            .await;

        let provider = AnthropicProvider::with_base_url(
            AnthropicAuthentication::ApiKey(SecretValue::new("sk-test")),
            DEFAULT_MODEL,
            server.uri(),
        );
        let mut request = TextRequest::new("sys", "usr");
        request.response_format = Some(ResponseFormat::Json {
            schema: Some(serde_json::json!({ "title": "Facts" })),
            strict: false,
        });

        let structured = provider.complete_structured(&request).await.unwrap();

        assert_eq!(structured.value["title"], "Requiem");
        assert_eq!(structured.input_tokens, 11);
        assert_eq!(structured.output_tokens, 5);
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
            AnthropicAuthentication::ApiKey(SecretValue::new("sk-test")),
            DEFAULT_MODEL,
            server.uri(),
        );
        let mut request = TextRequest::new("sys", "usr");
        request.response_format = Some(ResponseFormat::Json {
            schema: Some(serde_json::json!({ "title": "Facts" })),
            strict: false,
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
            AnthropicAuthentication::ApiKey(SecretValue::new("sk-test")),
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
    async fn the_key_is_never_carried_across_a_redirect() {
        let elsewhere = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&elsewhere)
            .await;
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(307)
                    .insert_header("location", format!("{}/v1/messages", elsewhere.uri())),
            )
            .mount(&server)
            .await;
        let provider = AnthropicProvider::with_base_url(
            AnthropicAuthentication::ApiKey(SecretValue::new(concat!(
                "sk-ant-",
                "api03-redirected"
            ))),
            DEFAULT_MODEL,
            server.uri(),
        );

        let error = provider
            .complete(&TextRequest::new("sys", "usr"))
            .await
            .unwrap_err();

        assert!(
            matches!(error, ProviderError::Api { status: 307, .. }),
            "{error:?}"
        );
        assert!(
            elsewhere.received_requests().await.unwrap().is_empty(),
            "the request, and its key, followed the redirect"
        );
    }

    #[tokio::test]
    async fn a_call_that_outlives_its_deadline_fails() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(5)))
            .mount(&server)
            .await;
        let provider = AnthropicProvider::with_base_url(
            AnthropicAuthentication::ApiKey(SecretValue::new("sk-test")),
            DEFAULT_MODEL,
            server.uri(),
        )
        .with_timeout(Duration::from_millis(200));

        let started = Instant::now();
        let error = provider
            .complete(&TextRequest::new("sys", "usr"))
            .await
            .unwrap_err();

        assert!(
            matches!(&error, ProviderError::Network { detail } if detail.contains("no answer within")),
            "{error:?}"
        );
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn the_cli_gets_the_token_in_its_environment_and_the_prompt_on_stdin() {
        let directory = tempfile::tempdir().unwrap();
        let executable = fake_cli(
            directory.path(),
            r#"printf '%s' '{"result":"from the cli","usage":{"input_tokens":3,"output_tokens":2}}'"#,
        );
        let provider =
            AnthropicProvider::with_oauth("oauth-token-7f3a").with_executable(executable);
        let prompt = format!("--help is not a flag here {}", "x".repeat(200 * 1024));

        let response = provider
            .complete(&TextRequest::new("", prompt.clone()))
            .await
            .unwrap();

        assert_eq!(response.content, "from the cli");
        assert_eq!(response.input_tokens, 3);
        let read = |name: &str| std::fs::read_to_string(directory.path().join(name)).unwrap();
        assert_eq!(read("token"), "oauth-token-7f3a");
        assert_eq!(read("stdin"), prompt);
        assert!(!read("arguments").contains("--help"));
        assert!(read("arguments").contains("--max-turns=1"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn the_bare_cli_is_given_no_token() {
        let directory = tempfile::tempdir().unwrap();
        let executable = fake_cli(directory.path(), r#"printf '%s' '{"result":"ok"}'"#);
        let provider =
            AnthropicProvider::with_model(AnthropicAuthentication::ClaudeCli, DEFAULT_MODEL)
                .with_executable(executable);

        provider
            .complete(&TextRequest::new("sys", "usr"))
            .await
            .unwrap();

        let token = std::fs::read_to_string(directory.path().join("token")).unwrap();
        assert!(token.is_empty(), "{token}");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_failing_cli_is_reported_as_an_exit_not_a_network_failure() {
        let directory = tempfile::tempdir().unwrap();
        let executable = fake_cli(directory.path(), "echo 'not signed in' >&2; exit 3");
        let provider = AnthropicProvider::with_oauth("token").with_executable(executable);

        let error = provider
            .complete(&TextRequest::new("sys", "usr"))
            .await
            .unwrap_err();

        assert!(
            matches!(&error, ProviderError::Exit { status: crate::provider::ExitStatus::Code(3), message, .. } if message.contains("not signed in")),
            "{error:?}"
        );
    }

    #[cfg(unix)]
    async fn cli_failure(body: &str) -> ProviderError {
        let directory = tempfile::tempdir().unwrap();
        let executable = fake_cli(directory.path(), body);
        AnthropicProvider::with_oauth("token")
            .with_executable(executable)
            .complete(&TextRequest::new("sys", "usr"))
            .await
            .unwrap_err()
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_failing_cli_reports_what_its_envelope_says_rather_than_its_diagnostics() {
        let error = cli_failure(
            r#"printf '%s' '{"type":"result","subtype":"success","is_error":true,"result":"Invalid API key. Please run /login"}'; echo 'unrelated noise' >&2; exit 1"#,
        )
        .await;

        assert!(
            matches!(&error, ProviderError::Exit { status: crate::provider::ExitStatus::Code(1), message, .. } if message.contains("Invalid API key") && !message.contains("noise")),
            "{error:?}"
        );
        assert!(!error.transient());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_throttled_cli_is_a_transient_api_failure() {
        let error = cli_failure(
            r#"printf '%s' '{"type":"result","is_error":true,"api_error_status":429,"result":"API Error: 429 {\"type\":\"error\",\"error\":{\"type\":\"rate_limit_error\"}}"}'; exit 1"#,
        )
        .await;

        assert!(
            matches!(&error, ProviderError::Http { provider, source: crate::Error::Api { status: 429, message } } if provider == NAME && message.contains("rate_limit_error")),
            "{error:?}"
        );
        assert!(error.transient());
        assert!(error.recoverable());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_status_named_only_in_the_result_is_read_from_it() {
        let error = cli_failure(
            r#"printf '%s' '{"is_error":true,"api_error_status":null,"result":"API Error: 529 Overloaded"}'; exit 1"#,
        )
        .await;

        assert!(
            matches!(
                &error,
                ProviderError::Http {
                    source: crate::Error::Api { status: 529, .. },
                    ..
                }
            ),
            "{error:?}"
        );
        assert!(error.transient());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn an_envelope_reporting_an_error_is_not_an_answer_even_on_a_clean_exit() {
        let error = cli_failure(
            r#"printf '%s' '{"type":"result","subtype":"success","is_error":true,"result":"API Error: 500 Internal server error"}'"#,
        )
        .await;

        assert!(
            matches!(
                &error,
                ProviderError::Http {
                    source: crate::Error::Api { status: 500, .. },
                    ..
                }
            ),
            "{error:?}"
        );
        assert!(error.transient());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_structured_call_whose_envelope_reports_an_error_fails() {
        let directory = tempfile::tempdir().unwrap();
        let executable = fake_cli(
            directory.path(),
            r#"printf '%s' '{"type":"result","subtype":"error_max_turns","is_error":true,"errors":["Reached maximum number of turns (1)"]}'"#,
        );
        let provider = AnthropicProvider::with_oauth("token").with_executable(executable);
        let mut request = TextRequest::new("sys", "usr");
        request.response_format = Some(ResponseFormat::Json {
            schema: Some(serde_json::json!({ "type": "object" })),
            strict: false,
        });

        let error = provider.complete_structured(&request).await.unwrap_err();

        assert!(
            matches!(&error, ProviderError::Agent { message, .. } if message.contains("maximum number of turns")),
            "{error:?}"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_credential_echoed_in_the_envelope_never_reaches_the_error() {
        let key = concat!("sk-ant-", "api03-", "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA");
        let error = cli_failure(&format!(
            r#"printf '%s' '{{"is_error":true,"result":"Invalid API key {key}"}}'; exit 1"#
        ))
        .await;

        let rendered = format!("{error} {error:?}");
        assert!(!rendered.contains(key), "{rendered}");
        assert!(rendered.contains("Invalid API key"), "{rendered}");
    }

    #[tokio::test]
    async fn a_missing_cli_is_reported_as_unavailable() {
        let provider = AnthropicProvider::with_oauth("token")
            .with_executable("/nonexistent/claude-for-this-test");

        let error = provider
            .complete(&TextRequest::new("sys", "usr"))
            .await
            .unwrap_err();

        assert!(
            matches!(error, ProviderError::Unavailable { .. }),
            "{error:?}"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_cli_that_overruns_its_deadline_is_stopped() {
        let directory = tempfile::tempdir().unwrap();
        let executable = fake_cli(directory.path(), "sleep 30");
        let provider = AnthropicProvider::with_oauth("token")
            .with_executable(executable)
            .with_timeout(Duration::from_millis(300));

        let started = Instant::now();
        let error = provider
            .complete(&TextRequest::new("sys", "usr"))
            .await
            .unwrap_err();

        assert!(matches!(error, ProviderError::Timeout { .. }), "{error:?}");
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_structured_answer_is_asked_of_the_cli_with_its_schema() {
        let directory = tempfile::tempdir().unwrap();
        let executable = fake_cli(
            directory.path(),
            r#"printf '%s' '{"structured_output":{"beats":3}}'"#,
        );
        let provider = AnthropicProvider::with_oauth("token").with_executable(executable);
        let mut request = TextRequest::new("sys", "usr");
        request.response_format = Some(ResponseFormat::Json {
            schema: Some(serde_json::json!({ "type": "object" })),
            strict: false,
        });

        let structured = provider.complete_structured(&request).await.unwrap();

        assert_eq!(structured.value["beats"], 3);
        let arguments = std::fs::read_to_string(directory.path().join("arguments")).unwrap();
        assert!(
            arguments.contains(r#"--json-schema={"type":"object"}"#),
            "{arguments}"
        );
        assert_eq!(
            std::fs::read_to_string(directory.path().join("stdin")).unwrap(),
            "sys\n\nusr"
        );
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
