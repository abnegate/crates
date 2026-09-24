//! The OpenAI chat, image, embedding and transcription APIs.

use std::path::Path;
use std::time::Duration;

use abnegate_secret::SecretValue;
use async_trait::async_trait;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use futures::Stream;

use crate::modality::vendor::transport::Transport;
use crate::modality::{
    EmbeddingProvider, ImageEditRequest, ImageProvider, ImageRequest, ImageResponse,
    ResponseFormat, StructuredResponse, TextProvider, TextRequest, TextResponse,
    TranscriptionProvider, TranscriptionResponse, TranscriptionSegment,
};
use crate::provider::ProviderError;

const BASE_URL: &str = "https://api.openai.com";
const DEFAULT_MODEL: &str = "gpt-5.4";
const EMBEDDING_DIMENSIONS: u32 = 768;
const EMBEDDING_MODEL: &str = "text-embedding-3-small";
const IMAGE_MODEL: &str = "dall-e-3";
const MAXIMUM_CONTEXT_TOKENS: u32 = 128_000;
const MAXIMUM_RESOLUTION: (u32, u32) = (1792, 1024);
const TRANSCRIPTION_MODEL: &str = "whisper-1";
/// Model families that reason before answering. OpenAI rejects `max_tokens`
/// and any `temperature` but the default for these.
const REASONING_FAMILIES: &[&str] = &["o1", "o3", "o4", "gpt-5"];
/// Model families that predate structured outputs. OpenAI refuses a
/// `json_schema` response format from these, so they are asked for
/// `json_object` instead.
const JSON_OBJECT_FAMILIES: &[&str] = &["gpt-3.5", "gpt-4", "gpt-4o-2024-05-13"];
const STRUCTURED_SCHEMA_NAME: &str = "response";

/// OpenAI over its REST API.
///
/// Models whose name belongs to a reasoning family (`o1`, `o3`, `o4` and
/// `gpt-5`, with any `-` or `.` suffix, optionally behind an `owner/` prefix)
/// are sent `max_completion_tokens` and no `temperature`, which is all those
/// models accept; every other model gets the legacy `max_tokens` and
/// `temperature` fields that older OpenAI-compatible servers expect.
///
/// A JSON schema is sent as a `json_schema` response format, strict only when
/// [`ResponseFormat::Json`] asks for it. `gpt-3.5` and `gpt-4` models, which
/// predate structured outputs, are asked for `json_object` instead.
///
/// Every call has a deadline, ten minutes unless [`Self::with_timeout`] says
/// otherwise, and the client never follows a redirect with the key.
#[derive(Debug, Clone)]
pub struct OpenAiProvider {
    api_key: SecretValue,
    model: String,
    base_url: String,
    transport: Transport,
}

impl OpenAiProvider {
    pub fn new(api_key: impl Into<SecretValue>) -> Self {
        Self::with_model(api_key, DEFAULT_MODEL)
    }

    pub fn with_model(api_key: impl Into<SecretValue>, model: &str) -> Self {
        Self::with_base_url(api_key, model, BASE_URL)
    }

    /// A provider that talks to `base_url` instead of the public API, for a
    /// proxy, a gateway or a test double.
    pub fn with_base_url(
        api_key: impl Into<SecretValue>,
        model: &str,
        base_url: impl Into<String>,
    ) -> Self {
        Self {
            api_key: api_key.into(),
            model: model.to_string(),
            base_url: base_url.into(),
            transport: Transport::default(),
        }
    }

    /// The deadline for one call.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.transport = Transport::with_timeout(timeout);
        self
    }

    pub fn build_chat_request_body(&self, request: &TextRequest) -> serde_json::Value {
        let mut body = serde_json::json!({
            "model": self.model,
            "messages": [
                { "role": "system", "content": request.system_prompt },
                { "role": "user", "content": request.user_prompt }
            ]
        });

        if is_reasoning_model(&self.model) {
            body["max_completion_tokens"] = request.maximum_tokens.into();
        } else {
            body["max_tokens"] = request.maximum_tokens.into();
            body["temperature"] = request.temperature.into();
        }

        match &request.response_format {
            Some(ResponseFormat::Json {
                schema: Some(schema),
                strict,
            }) if takes_json_schema(&self.model) => {
                let mut format = serde_json::json!({
                    "name": STRUCTURED_SCHEMA_NAME,
                    "schema": schema
                });
                if *strict {
                    format["strict"] = true.into();
                }
                body["response_format"] = serde_json::json!({
                    "type": "json_schema",
                    "json_schema": format
                });
            }
            Some(ResponseFormat::Json { .. }) => {
                body["response_format"] = serde_json::json!({ "type": "json_object" });
            }
            Some(ResponseFormat::Text) | None => {}
        }

        body
    }

    pub fn build_image_request_body(&self, request: &ImageRequest) -> serde_json::Value {
        let mut body = serde_json::json!({
            "model": IMAGE_MODEL,
            "prompt": request.prompt,
            "n": request.image_count,
            "size": format!("{}x{}", request.width, request.height),
            "response_format": "b64_json"
        });

        if let Some(style) = &request.style
            && let Some(object) = body.as_object_mut()
        {
            object.insert("style".into(), serde_json::Value::String(style.clone()));
        }

        body
    }

    pub fn build_embedding_request_body(&self, texts: &[String]) -> serde_json::Value {
        serde_json::json!({
            "model": EMBEDDING_MODEL,
            "input": texts,
            "dimensions": EMBEDDING_DIMENSIONS
        })
    }

    pub fn parse_chat_response(body: &serde_json::Value) -> Result<TextResponse, ProviderError> {
        let choice = body
            .get("choices")
            .and_then(|choices| choices.as_array())
            .and_then(|choices| choices.first());

        let content = choice
            .and_then(|choice| choice.get("message"))
            .and_then(|message| message.get("content"))
            .and_then(|content| content.as_str())
            .ok_or_else(|| ProviderError::parse("missing choices[0].message.content in response"))?
            .to_string();

        Ok(TextResponse {
            content,
            model: body
                .get("model")
                .and_then(|model| model.as_str())
                .unwrap_or("unknown")
                .to_string(),
            input_tokens: usage(body, "prompt_tokens"),
            output_tokens: usage(body, "completion_tokens"),
            finish_reason: choice
                .and_then(|choice| choice.get("finish_reason"))
                .and_then(|reason| reason.as_str())
                .unwrap_or("stop")
                .to_string(),
        })
    }

    fn post(&self, path: &str) -> reqwest::RequestBuilder {
        self.transport
            .post(format!("{}{path}", self.base_url))
            .bearer_auth(self.api_key.expose())
    }

    async fn send_json(
        &self,
        path: &str,
        body: &serde_json::Value,
    ) -> Result<serde_json::Value, ProviderError> {
        self.transport.send(self.post(path).json(body)).await
    }
}

fn is_reasoning_model(model: &str) -> bool {
    belongs_to(model, REASONING_FAMILIES, &['-', '.'])
}

fn takes_json_schema(model: &str) -> bool {
    !belongs_to(model, JSON_OBJECT_FAMILIES, &['-'])
}

/// Whether `model`, after any `owner/` prefix, is one of `families` itself or
/// one of them followed by a `separator`.
fn belongs_to(model: &str, families: &[&str], separators: &[char]) -> bool {
    let name = model
        .rsplit('/')
        .next()
        .unwrap_or(model)
        .to_ascii_lowercase();
    families.iter().any(|family| {
        name.strip_prefix(family)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with(separators))
    })
}

fn usage(body: &serde_json::Value, field: &str) -> u32 {
    body.get("usage")
        .and_then(|usage| usage.get(field))
        .and_then(|count| count.as_u64())
        .map_or(0, |count| u32::try_from(count).unwrap_or(u32::MAX))
}

#[async_trait]
impl TextProvider for OpenAiProvider {
    fn name(&self) -> &str {
        "openai"
    }

    fn supports_structured_output(&self) -> bool {
        true
    }

    fn maximum_context_tokens(&self) -> u32 {
        MAXIMUM_CONTEXT_TOKENS
    }

    async fn complete(&self, request: &TextRequest) -> Result<TextResponse, ProviderError> {
        let body = self.build_chat_request_body(request);
        let json = self.send_json("/v1/chat/completions", &body).await?;
        Self::parse_chat_response(&json)
    }

    async fn complete_structured(
        &self,
        request: &TextRequest,
    ) -> Result<StructuredResponse, ProviderError> {
        StructuredResponse::from_text(self.complete(request).await?)
    }

    async fn stream_complete(
        &self,
        _request: &TextRequest,
    ) -> Result<Box<dyn Stream<Item = Result<String, ProviderError>> + Send + Unpin>, ProviderError>
    {
        Err(ProviderError::unsupported(
            "streaming not yet implemented for OpenAI provider",
        ))
    }
}

#[async_trait]
impl ImageProvider for OpenAiProvider {
    fn name(&self) -> &str {
        "openai"
    }

    fn supported_styles(&self) -> Vec<String> {
        vec!["vivid".into(), "natural".into()]
    }

    fn maximum_resolution(&self) -> (u32, u32) {
        MAXIMUM_RESOLUTION
    }

    async fn generate(&self, request: &ImageRequest) -> Result<ImageResponse, ProviderError> {
        let body = self.build_image_request_body(request);
        let json = self.send_json("/v1/images/generations", &body).await?;

        let image = json
            .get("data")
            .and_then(|data| data.as_array())
            .and_then(|data| data.first())
            .ok_or_else(|| ProviderError::parse("missing data[0] in response"))?;

        let encoded = image
            .get("b64_json")
            .and_then(|encoded| encoded.as_str())
            .ok_or_else(|| ProviderError::parse("missing data[0].b64_json in response"))?;

        Ok(ImageResponse {
            data: STANDARD
                .decode(encoded)
                .map_err(|error| ProviderError::parse(format!("base64 decode error: {error}")))?,
            width: request.width,
            height: request.height,
            format: "png".into(),
            revised_prompt: image
                .get("revised_prompt")
                .and_then(|prompt| prompt.as_str())
                .map(str::to_string),
        })
    }

    async fn edit(&self, _request: &ImageEditRequest) -> Result<ImageResponse, ProviderError> {
        Err(ProviderError::unsupported(
            "image editing not yet implemented for OpenAI provider",
        ))
    }

    async fn variations(
        &self,
        _image: &[u8],
        _count: u32,
    ) -> Result<Vec<ImageResponse>, ProviderError> {
        Err(ProviderError::unsupported(
            "image variations not yet implemented for OpenAI provider",
        ))
    }
}

#[async_trait]
impl EmbeddingProvider for OpenAiProvider {
    fn name(&self) -> &str {
        "openai"
    }

    fn dimensions(&self) -> u32 {
        EMBEDDING_DIMENSIONS
    }

    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, ProviderError> {
        let body = self.build_embedding_request_body(texts);
        let json = self.send_json("/v1/embeddings", &body).await?;

        let data = json
            .get("data")
            .and_then(|data| data.as_array())
            .ok_or_else(|| ProviderError::parse("missing data array in response"))?;

        let mut embeddings = Vec::with_capacity(data.len());
        for item in data {
            let embedding = item
                .get("embedding")
                .and_then(|embedding| embedding.as_array())
                .ok_or_else(|| ProviderError::parse("missing embedding in data item"))?
                .iter()
                .filter_map(|value| value.as_f64().map(|value| value as f32))
                .collect();
            embeddings.push(embedding);
        }

        Ok(embeddings)
    }

    async fn embed_single(&self, text: &str) -> Result<Vec<f32>, ProviderError> {
        self.embed(&[text.to_string()])
            .await?
            .pop()
            .ok_or_else(|| ProviderError::parse("empty embedding response"))
    }
}

#[async_trait]
impl TranscriptionProvider for OpenAiProvider {
    fn name(&self) -> &str {
        "openai"
    }

    async fn transcribe(&self, audio_path: &Path) -> Result<TranscriptionResponse, ProviderError> {
        let bytes = tokio::fs::read(audio_path)
            .await
            .map_err(|error| ProviderError::io(error.to_string()))?;

        let file_name = audio_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("audio.mp3")
            .to_string();

        let part = reqwest::multipart::Part::bytes(bytes)
            .file_name(file_name)
            .mime_str("audio/mpeg")
            .map_err(|error| ProviderError::config(error.without_url()))?;

        let form = reqwest::multipart::Form::new()
            .text("model", TRANSCRIPTION_MODEL)
            .text("response_format", "verbose_json")
            .part("file", part);

        let json = self
            .transport
            .send(self.post("/v1/audio/transcriptions").multipart(form))
            .await?;

        Ok(TranscriptionResponse {
            text: json
                .get("text")
                .and_then(|text| text.as_str())
                .unwrap_or("")
                .to_string(),
            segments: json
                .get("segments")
                .and_then(|segments| segments.as_array())
                .map(|segments| segments.iter().map(parse_segment).collect())
                .unwrap_or_default(),
            language: json
                .get("language")
                .and_then(|language| language.as_str())
                .unwrap_or("en")
                .to_string(),
            duration_seconds: json
                .get("duration")
                .and_then(|duration| duration.as_f64())
                .unwrap_or(0.0),
        })
    }
}

fn parse_segment(segment: &serde_json::Value) -> TranscriptionSegment {
    TranscriptionSegment {
        start: segment
            .get("start")
            .and_then(|start| start.as_f64())
            .unwrap_or(0.0),
        end: segment
            .get("end")
            .and_then(|end| end.as_f64())
            .unwrap_or(0.0),
        text: segment
            .get("text")
            .and_then(|text| text.as_str())
            .unwrap_or("")
            .to_string(),
        confidence: segment
            .get("avg_logprob")
            .and_then(|confidence| confidence.as_f64())
            .unwrap_or(0.0),
    }
}

#[cfg(test)]
mod tests {
    use wiremock::matchers::{body_partial_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    fn request() -> TextRequest {
        TextRequest {
            system_prompt: "You are a coding assistant.".into(),
            user_prompt: "Write hello world in Rust.".into(),
            temperature: 0.5,
            maximum_tokens: 2048,
            response_format: None,
            context: None,
        }
    }

    #[test]
    fn a_chat_body_carries_the_model_the_settings_and_both_messages() {
        let provider = OpenAiProvider::with_model("test-key", "gpt-4o");

        let body = provider.build_chat_request_body(&request());

        assert_eq!(body["model"], "gpt-4o");
        assert_eq!(body["max_tokens"], 2048);
        assert_eq!(body["temperature"], 0.5);
        assert!(body.get("max_completion_tokens").is_none());

        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["role"], "system");
        assert_eq!(messages[0]["content"], "You are a coding assistant.");
        assert_eq!(messages[1]["role"], "user");
        assert_eq!(messages[1]["content"], "Write hello world in Rust.");
    }

    #[test]
    fn a_custom_model_reaches_the_body() {
        let provider = OpenAiProvider::with_model("key", "gpt-4-turbo");
        let body = provider.build_chat_request_body(&TextRequest::new("sys", "usr"));
        assert_eq!(body["model"], "gpt-4-turbo");
    }

    #[test]
    fn the_default_reasoning_model_gets_only_the_fields_it_accepts() {
        let body = OpenAiProvider::new("key").build_chat_request_body(&request());

        assert_eq!(body["model"], DEFAULT_MODEL);
        assert_eq!(body["max_completion_tokens"], 2048);
        assert!(body.get("max_tokens").is_none(), "{body}");
        assert!(body.get("temperature").is_none(), "{body}");
    }

    #[test]
    fn a_structured_request_to_a_reasoning_model_keeps_its_schema() {
        let mut request = request();
        request.response_format = Some(ResponseFormat::Json {
            schema: Some(serde_json::json!({ "type": "object" })),
            strict: false,
        });

        let body = OpenAiProvider::with_model("key", "o3-mini").build_chat_request_body(&request);

        assert_eq!(body["response_format"]["type"], "json_schema");
        assert_eq!(
            body["response_format"]["json_schema"]["schema"]["type"],
            "object"
        );
        assert!(body.get("temperature").is_none(), "{body}");
        assert!(body.get("max_tokens").is_none(), "{body}");
    }

    fn structured(schema_strict: bool) -> TextRequest {
        let mut request = request();
        request.response_format = Some(ResponseFormat::Json {
            schema: Some(serde_json::json!({ "type": "object" })),
            strict: schema_strict,
        });
        request
    }

    #[test]
    fn a_schema_is_not_sent_strict_unless_the_caller_asks() {
        let body =
            OpenAiProvider::with_model("key", "gpt-4o").build_chat_request_body(&structured(false));

        assert_eq!(body["response_format"]["type"], "json_schema");
        assert_eq!(
            body["response_format"]["json_schema"]["schema"]["type"],
            "object"
        );
        assert!(
            body["response_format"]["json_schema"]
                .get("strict")
                .is_none(),
            "{body}"
        );
    }

    #[test]
    fn a_strict_schema_is_sent_strict() {
        let body =
            OpenAiProvider::with_model("key", "gpt-4o").build_chat_request_body(&structured(true));

        assert_eq!(body["response_format"]["type"], "json_schema");
        assert_eq!(body["response_format"]["json_schema"]["strict"], true);
    }

    #[test]
    fn a_model_that_predates_structured_outputs_is_asked_for_a_json_object() {
        for model in [
            "gpt-4",
            "gpt-4-turbo",
            "gpt-4-0613",
            "gpt-3.5-turbo",
            "openai/gpt-3.5-turbo-0125",
            "gpt-4o-2024-05-13",
        ] {
            for strict in [false, true] {
                let body = OpenAiProvider::with_model("key", model)
                    .build_chat_request_body(&structured(strict));
                assert_eq!(
                    body["response_format"],
                    serde_json::json!({ "type": "json_object" }),
                    "{model}"
                );
            }
        }
        for model in [
            "gpt-4o",
            "gpt-4o-mini",
            "gpt-4.1",
            "gpt-5.4",
            "o3-mini",
            "llama3",
        ] {
            let body = OpenAiProvider::with_model("key", model)
                .build_chat_request_body(&structured(false));
            assert_eq!(body["response_format"]["type"], "json_schema", "{model}");
        }
    }

    #[test]
    fn reasoning_families_are_recognised_by_name() {
        for model in [
            "o1",
            "o1-mini",
            "o3",
            "o3-mini-2025-01-31",
            "o4-mini",
            "gpt-5",
            "gpt-5.4",
            "gpt-5-mini",
            "openai/gpt-5.4",
            "GPT-5.4",
        ] {
            assert!(is_reasoning_model(model), "{model}");
        }
        for model in [
            "gpt-4", "gpt-4o", "gpt-4.1", "gpt-50", "o10", "omni", "llama-o3",
        ] {
            assert!(!is_reasoning_model(model), "{model}");
        }
    }

    #[test]
    fn asking_for_json_sets_the_response_format() {
        let provider = OpenAiProvider::new("key");
        let mut request = request();
        request.response_format = Some(ResponseFormat::Json {
            schema: None,
            strict: false,
        });

        let body = provider.build_chat_request_body(&request);

        assert_eq!(body["response_format"]["type"], "json_object");
    }

    #[test]
    fn asking_for_text_sets_no_response_format() {
        let provider = OpenAiProvider::new("key");
        let mut request = request();
        request.response_format = Some(ResponseFormat::Text);

        let body = provider.build_chat_request_body(&request);

        assert!(body.get("response_format").is_none());
    }

    #[test]
    fn an_image_body_carries_the_size_the_count_and_the_style() {
        let provider = OpenAiProvider::new("test-key");
        let request = ImageRequest {
            prompt: "A sunset over mountains".into(),
            negative_prompt: None,
            width: 1024,
            height: 1024,
            style: Some("vivid".into()),
            reference_images: Vec::new(),
            image_count: 1,
        };

        let body = provider.build_image_request_body(&request);

        assert_eq!(body["model"], IMAGE_MODEL);
        assert_eq!(body["prompt"], "A sunset over mountains");
        assert_eq!(body["n"], 1);
        assert_eq!(body["size"], "1024x1024");
        assert_eq!(body["response_format"], "b64_json");
        assert_eq!(body["style"], "vivid");
    }

    #[test]
    fn an_image_body_without_a_style_omits_the_field() {
        let provider = OpenAiProvider::new("key");
        let request = ImageRequest::new("A cat", 512, 512);

        let body = provider.build_image_request_body(&request);

        assert!(body.get("style").is_none());
        assert_eq!(body["n"], 1);
        assert_eq!(body["size"], "512x512");
    }

    #[test]
    fn an_embedding_body_carries_every_text() {
        let provider = OpenAiProvider::new("test-key");

        let body =
            provider.build_embedding_request_body(&["Hello world".into(), "Goodbye world".into()]);

        assert_eq!(body["model"], EMBEDDING_MODEL);
        assert_eq!(body["dimensions"], EMBEDDING_DIMENSIONS);
        let input = body["input"].as_array().unwrap();
        assert_eq!(input.len(), 2);
        assert_eq!(input[0], "Hello world");
        assert_eq!(input[1], "Goodbye world");
    }

    #[test]
    fn an_embedding_body_for_no_texts_is_an_empty_list() {
        let provider = OpenAiProvider::new("key");
        let body = provider.build_embedding_request_body(&[]);
        assert_eq!(body["input"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn a_chat_response_is_read_in_full() {
        let body = serde_json::json!({
            "id": "chatcmpl-abc123",
            "object": "chat.completion",
            "model": "gpt-4o-2024-05-13",
            "choices": [{
                "index": 0,
                "message": { "role": "assistant", "content": "fn main() {}" },
                "finish_reason": "stop"
            }],
            "usage": { "prompt_tokens": 30, "completion_tokens": 20, "total_tokens": 50 }
        });

        let response = OpenAiProvider::parse_chat_response(&body).unwrap();

        assert_eq!(response.content, "fn main() {}");
        assert_eq!(response.model, "gpt-4o-2024-05-13");
        assert_eq!(response.input_tokens, 30);
        assert_eq!(response.output_tokens, 20);
        assert_eq!(response.finish_reason, "stop");
    }

    #[test]
    fn a_response_without_content_is_an_error_not_an_empty_answer() {
        assert!(OpenAiProvider::parse_chat_response(&serde_json::json!({ "id": "x" })).is_err());
        assert!(
            OpenAiProvider::parse_chat_response(&serde_json::json!({ "choices": [] })).is_err()
        );
    }

    #[test]
    fn the_optional_parts_of_a_response_have_defaults() {
        let body = serde_json::json!({
            "choices": [{ "message": { "role": "assistant", "content": "hello" } }]
        });

        let response = OpenAiProvider::parse_chat_response(&body).unwrap();

        assert_eq!(response.model, "unknown");
        assert_eq!(response.input_tokens, 0);
        assert_eq!(response.output_tokens, 0);
        assert_eq!(response.finish_reason, "stop");
    }

    #[test]
    fn the_provider_reports_what_it_can_do() {
        let provider = OpenAiProvider::new("key");

        assert_eq!(TextProvider::name(&provider), "openai");
        assert_eq!(ImageProvider::name(&provider), "openai");
        assert_eq!(EmbeddingProvider::name(&provider), "openai");
        assert_eq!(TranscriptionProvider::name(&provider), "openai");
        assert!(provider.supports_structured_output());
        assert_eq!(provider.maximum_context_tokens(), MAXIMUM_CONTEXT_TOKENS);
        assert_eq!(provider.maximum_resolution(), MAXIMUM_RESOLUTION);
        assert_eq!(provider.dimensions(), EMBEDDING_DIMENSIONS);
        let styles = provider.supported_styles();
        assert!(styles.contains(&"vivid".to_string()));
        assert!(styles.contains(&"natural".to_string()));
    }

    #[tokio::test]
    async fn a_completion_is_sent_authenticated_and_read_back() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .and(header("Authorization", "Bearer sk-test"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "model": "gpt-5.4",
                "choices": [{
                    "message": { "role": "assistant", "content": "hello" },
                    "finish_reason": "stop"
                }],
                "usage": { "prompt_tokens": 7, "completion_tokens": 3 }
            })))
            .mount(&server)
            .await;

        let provider = OpenAiProvider::with_base_url("sk-test", DEFAULT_MODEL, server.uri());
        let response = provider
            .complete(&TextRequest::new("sys", "usr"))
            .await
            .unwrap();

        assert_eq!(response.content, "hello");
        assert_eq!(response.input_tokens, 7);
        assert_eq!(response.output_tokens, 3);
    }

    #[tokio::test]
    async fn a_structured_answer_is_requested_against_its_schema_and_parsed() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .and(body_partial_json(serde_json::json!({
                "max_completion_tokens": 4096,
                "response_format": { "type": "json_schema" }
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "choices": [{ "message": { "role": "assistant", "content": r#"{"beats":3}"# } }]
            })))
            .expect(1)
            .mount(&server)
            .await;
        let provider = OpenAiProvider::with_base_url("sk-test", DEFAULT_MODEL, server.uri());
        let mut request = TextRequest::new("sys", "usr");
        request.response_format = Some(ResponseFormat::Json {
            schema: Some(serde_json::json!({ "type": "object" })),
            strict: false,
        });

        let structured = provider.complete_structured(&request).await.unwrap();

        assert_eq!(structured.value["beats"], 3);
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
            .respond_with(ResponseTemplate::new(307).insert_header(
                "location",
                format!("{}/v1/chat/completions", elsewhere.uri()),
            ))
            .mount(&server)
            .await;

        let error = OpenAiProvider::with_base_url("sk-test", DEFAULT_MODEL, server.uri())
            .complete(&TextRequest::new("sys", "usr"))
            .await
            .unwrap_err();

        assert!(
            matches!(error, ProviderError::Api { status: 307, .. }),
            "{error:?}"
        );
        assert!(elsewhere.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_refused_call_carries_the_status_and_the_body() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(429).set_body_string("rate limited"))
            .mount(&server)
            .await;

        let provider = OpenAiProvider::with_base_url("sk-test", DEFAULT_MODEL, server.uri());
        let error = provider
            .complete(&TextRequest::new("sys", "usr"))
            .await
            .unwrap_err();

        match error {
            ProviderError::Api { status, message } => {
                assert_eq!(status, 429);
                assert_eq!(message, "rate limited");
            }
            other => panic!("expected ApiError, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_generated_image_arrives_decoded() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/images/generations"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [{
                    "b64_json": STANDARD.encode([1u8, 2, 3]),
                    "revised_prompt": "a revised prompt"
                }]
            })))
            .mount(&server)
            .await;

        let provider = OpenAiProvider::with_base_url("sk-test", DEFAULT_MODEL, server.uri());
        let response = provider
            .generate(&ImageRequest::new("A cat", 512, 512))
            .await
            .unwrap();

        assert_eq!(response.data, vec![1, 2, 3]);
        assert_eq!(response.width, 512);
        assert_eq!(response.format, "png");
        assert_eq!(response.revised_prompt.as_deref(), Some("a revised prompt"));
    }

    #[tokio::test]
    async fn embeddings_come_back_one_vector_per_text() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/embeddings"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [
                    { "embedding": [0.5, -0.25] },
                    { "embedding": [1.0, 0.0] }
                ]
            })))
            .mount(&server)
            .await;

        let provider = OpenAiProvider::with_base_url("sk-test", DEFAULT_MODEL, server.uri());
        let embeddings = provider
            .embed(&["one".to_string(), "two".to_string()])
            .await
            .unwrap();

        assert_eq!(embeddings.len(), 2);
        assert_eq!(embeddings[0], vec![0.5, -0.25]);
        assert_eq!(embeddings[1], vec![1.0, 0.0]);
    }

    #[tokio::test]
    async fn an_audio_file_is_uploaded_and_its_segments_read_back() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/audio/transcriptions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "text": "Hello world",
                "language": "en",
                "duration": 1.5,
                "segments": [{
                    "start": 0.0,
                    "end": 1.5,
                    "text": "Hello world",
                    "avg_logprob": -0.25
                }]
            })))
            .mount(&server)
            .await;

        let audio = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(audio.path(), [0u8, 1, 2]).unwrap();

        let provider = OpenAiProvider::with_base_url("sk-test", DEFAULT_MODEL, server.uri());
        let response = provider.transcribe(audio.path()).await.unwrap();

        assert_eq!(response.text, "Hello world");
        assert_eq!(response.language, "en");
        assert_eq!(response.segments.len(), 1);
        assert!((response.segments[0].confidence + 0.25).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn a_missing_audio_file_is_an_io_error() {
        let provider = OpenAiProvider::new("key");
        let error = provider
            .transcribe(Path::new("/nonexistent/audio.mp3"))
            .await
            .unwrap_err();
        assert!(matches!(error, ProviderError::Io { .. }), "{error:?}");
    }

    #[tokio::test]
    async fn the_unimplemented_paths_say_so_rather_than_failing_obscurely() {
        let provider = OpenAiProvider::new("key");

        let streaming = provider
            .stream_complete(&TextRequest::new("sys", "usr"))
            .await
            .err()
            .unwrap();
        assert!(matches!(streaming, ProviderError::Unsupported { .. }));

        let edit = provider
            .edit(&ImageEditRequest {
                image: Vec::new(),
                mask: None,
                prompt: "x".into(),
                width: 1,
                height: 1,
            })
            .await
            .err()
            .unwrap();
        assert!(matches!(edit, ProviderError::Unsupported { .. }));

        let variations = provider.variations(&[], 1).await.err().unwrap();
        assert!(matches!(variations, ProviderError::Unsupported { .. }));
    }
}
