use std::path::Path;

use abnegate_secret::SecretValue;
use async_trait::async_trait;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use futures::Stream;

use crate::modality::{
    EmbeddingProvider, ImageEditRequest, ImageProvider, ImageRequest, ImageResponse, ModalityError,
    ResponseFormat, TextProvider, TextRequest, TextResponse, TranscriptionProvider,
    TranscriptionResponse, TranscriptionSegment,
};

const BASE_URL: &str = "https://api.openai.com";
const DEFAULT_MODEL: &str = "gpt-5.4";
const EMBEDDING_DIMENSIONS: u32 = 768;
const EMBEDDING_MODEL: &str = "text-embedding-3-small";
const IMAGE_MODEL: &str = "dall-e-3";
const MAX_CONTEXT_TOKENS: u32 = 128_000;
const MAX_RESOLUTION: (u32, u32) = (1792, 1024);
const TRANSCRIPTION_MODEL: &str = "whisper-1";

pub struct OpenAIProvider {
    api_key: SecretValue,
    model: String,
    base_url: String,
    client: reqwest::Client,
}

impl OpenAIProvider {
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
            client: reqwest::Client::new(),
        }
    }

    pub fn build_chat_request_body(&self, request: &TextRequest) -> serde_json::Value {
        let mut body = serde_json::json!({
            "model": self.model,
            "max_tokens": request.max_tokens,
            "temperature": request.temperature,
            "messages": [
                { "role": "system", "content": request.system_prompt },
                { "role": "user", "content": request.user_prompt }
            ]
        });

        if let Some(ResponseFormat::Json { .. }) = request.response_format
            && let Some(object) = body.as_object_mut()
        {
            object.insert(
                "response_format".into(),
                serde_json::json!({ "type": "json_object" }),
            );
        }

        body
    }

    pub fn build_image_request_body(&self, request: &ImageRequest) -> serde_json::Value {
        let mut body = serde_json::json!({
            "model": IMAGE_MODEL,
            "prompt": request.prompt,
            "n": request.num_images,
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

    pub fn parse_chat_response(body: &serde_json::Value) -> Result<TextResponse, ModalityError> {
        let choice = body
            .get("choices")
            .and_then(|choices| choices.as_array())
            .and_then(|choices| choices.first());

        let content = choice
            .and_then(|choice| choice.get("message"))
            .and_then(|message| message.get("content"))
            .and_then(|content| content.as_str())
            .ok_or_else(|| {
                ModalityError::ParseError("missing choices[0].message.content in response".into())
            })?
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

    fn post(&self, path: &str, body: &serde_json::Value) -> reqwest::RequestBuilder {
        self.client
            .post(format!("{}{path}", self.base_url))
            .header("Authorization", format!("Bearer {}", self.api_key.expose()))
            .header("Content-Type", "application/json")
            .json(body)
    }

    async fn send(request: reqwest::RequestBuilder) -> Result<serde_json::Value, ModalityError> {
        let response = request
            .send()
            .await
            .map_err(|error| ModalityError::NetworkError(error.to_string()))?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let message = response
                .text()
                .await
                .unwrap_or_else(|_| "unknown error".into());
            return Err(ModalityError::ApiError { status, message });
        }

        response
            .json()
            .await
            .map_err(|error| ModalityError::ParseError(error.to_string()))
    }
}

fn usage(body: &serde_json::Value, field: &str) -> u32 {
    body.get("usage")
        .and_then(|usage| usage.get(field))
        .and_then(|count| count.as_u64())
        .unwrap_or(0) as u32
}

#[async_trait]
impl TextProvider for OpenAIProvider {
    fn name(&self) -> &str {
        "openai"
    }

    fn supports_structured_output(&self) -> bool {
        true
    }

    fn max_context_tokens(&self) -> u32 {
        MAX_CONTEXT_TOKENS
    }

    async fn complete(&self, request: &TextRequest) -> Result<TextResponse, ModalityError> {
        let body = self.build_chat_request_body(request);
        let json = Self::send(self.post("/v1/chat/completions", &body)).await?;
        Self::parse_chat_response(&json)
    }

    async fn complete_structured(
        &self,
        request: &TextRequest,
    ) -> Result<serde_json::Value, ModalityError> {
        let Some(ResponseFormat::Json {
            schema: Some(schema),
        }) = &request.response_format
        else {
            let response = self.complete(request).await?;
            return parse_content(&response.content);
        };

        let body = serde_json::json!({
            "model": self.model,
            "max_tokens": request.max_tokens,
            "temperature": request.temperature,
            "messages": [
                { "role": "system", "content": request.system_prompt },
                { "role": "user", "content": request.user_prompt }
            ],
            "response_format": {
                "type": "json_schema",
                "json_schema": {
                    "name": "response",
                    "strict": true,
                    "schema": schema
                }
            }
        });

        let json = Self::send(self.post("/v1/chat/completions", &body)).await?;
        parse_content(&Self::parse_chat_response(&json)?.content)
    }

    async fn stream_complete(
        &self,
        _request: &TextRequest,
    ) -> Result<Box<dyn Stream<Item = Result<String, ModalityError>> + Send + Unpin>, ModalityError>
    {
        Err(ModalityError::Unsupported(
            "streaming not yet implemented for OpenAI provider".into(),
        ))
    }
}

fn parse_content(content: &str) -> Result<serde_json::Value, ModalityError> {
    serde_json::from_str(content).map_err(|error| {
        ModalityError::ParseError(format!("failed to parse structured output: {error}"))
    })
}

#[async_trait]
impl ImageProvider for OpenAIProvider {
    fn name(&self) -> &str {
        "openai"
    }

    fn supported_styles(&self) -> Vec<String> {
        vec!["vivid".into(), "natural".into()]
    }

    fn max_resolution(&self) -> (u32, u32) {
        MAX_RESOLUTION
    }

    async fn generate(&self, request: &ImageRequest) -> Result<ImageResponse, ModalityError> {
        let body = self.build_image_request_body(request);
        let json = Self::send(self.post("/v1/images/generations", &body)).await?;

        let image = json
            .get("data")
            .and_then(|data| data.as_array())
            .and_then(|data| data.first())
            .ok_or_else(|| ModalityError::ParseError("missing data[0] in response".into()))?;

        let encoded = image
            .get("b64_json")
            .and_then(|encoded| encoded.as_str())
            .ok_or_else(|| {
                ModalityError::ParseError("missing data[0].b64_json in response".into())
            })?;

        Ok(ImageResponse {
            data: STANDARD.decode(encoded).map_err(|error| {
                ModalityError::ParseError(format!("base64 decode error: {error}"))
            })?,
            width: request.width,
            height: request.height,
            format: "png".into(),
            revised_prompt: image
                .get("revised_prompt")
                .and_then(|prompt| prompt.as_str())
                .map(str::to_string),
        })
    }

    async fn edit(&self, _request: &ImageEditRequest) -> Result<ImageResponse, ModalityError> {
        Err(ModalityError::Unsupported(
            "image editing not yet implemented for OpenAI provider".into(),
        ))
    }

    async fn variations(
        &self,
        _image: &[u8],
        _count: u32,
    ) -> Result<Vec<ImageResponse>, ModalityError> {
        Err(ModalityError::Unsupported(
            "image variations not yet implemented for OpenAI provider".into(),
        ))
    }
}

#[async_trait]
impl EmbeddingProvider for OpenAIProvider {
    fn name(&self) -> &str {
        "openai"
    }

    fn dimensions(&self) -> u32 {
        EMBEDDING_DIMENSIONS
    }

    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, ModalityError> {
        let body = self.build_embedding_request_body(texts);
        let json = Self::send(self.post("/v1/embeddings", &body)).await?;

        let data = json
            .get("data")
            .and_then(|data| data.as_array())
            .ok_or_else(|| ModalityError::ParseError("missing data array in response".into()))?;

        let mut embeddings = Vec::with_capacity(data.len());
        for item in data {
            let embedding = item
                .get("embedding")
                .and_then(|embedding| embedding.as_array())
                .ok_or_else(|| ModalityError::ParseError("missing embedding in data item".into()))?
                .iter()
                .filter_map(|value| value.as_f64().map(|value| value as f32))
                .collect();
            embeddings.push(embedding);
        }

        Ok(embeddings)
    }

    async fn embed_single(&self, text: &str) -> Result<Vec<f32>, ModalityError> {
        self.embed(&[text.to_string()])
            .await?
            .pop()
            .ok_or_else(|| ModalityError::ParseError("empty embedding response".into()))
    }
}

#[async_trait]
impl TranscriptionProvider for OpenAIProvider {
    fn name(&self) -> &str {
        "openai"
    }

    async fn transcribe(&self, audio_path: &Path) -> Result<TranscriptionResponse, ModalityError> {
        let bytes = tokio::fs::read(audio_path)
            .await
            .map_err(|error| ModalityError::IoError(error.to_string()))?;

        let file_name = audio_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("audio.mp3")
            .to_string();

        let part = reqwest::multipart::Part::bytes(bytes)
            .file_name(file_name)
            .mime_str("audio/mpeg")
            .map_err(|error| ModalityError::NetworkError(error.to_string()))?;

        let form = reqwest::multipart::Form::new()
            .text("model", TRANSCRIPTION_MODEL)
            .text("response_format", "verbose_json")
            .part("file", part);

        let request = self
            .client
            .post(format!("{}/v1/audio/transcriptions", self.base_url))
            .header("Authorization", format!("Bearer {}", self.api_key.expose()))
            .multipart(form);

        let json = Self::send(request).await?;

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
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    fn request() -> TextRequest {
        TextRequest {
            system_prompt: "You are a coding assistant.".into(),
            user_prompt: "Write hello world in Rust.".into(),
            temperature: 0.5,
            max_tokens: 2048,
            response_format: None,
            context: None,
        }
    }

    #[test]
    fn a_chat_body_carries_the_model_the_settings_and_both_messages() {
        let provider = OpenAIProvider::new("test-key");

        let body = provider.build_chat_request_body(&request());

        assert_eq!(body["model"], DEFAULT_MODEL);
        assert_eq!(body["max_tokens"], 2048);
        assert_eq!(body["temperature"], 0.5);

        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["role"], "system");
        assert_eq!(messages[0]["content"], "You are a coding assistant.");
        assert_eq!(messages[1]["role"], "user");
        assert_eq!(messages[1]["content"], "Write hello world in Rust.");
    }

    #[test]
    fn a_custom_model_reaches_the_body() {
        let provider = OpenAIProvider::with_model("key", "gpt-4-turbo");
        let body = provider.build_chat_request_body(&TextRequest::new("sys", "usr"));
        assert_eq!(body["model"], "gpt-4-turbo");
    }

    #[test]
    fn asking_for_json_sets_the_response_format() {
        let provider = OpenAIProvider::new("key");
        let mut request = request();
        request.response_format = Some(ResponseFormat::Json { schema: None });

        let body = provider.build_chat_request_body(&request);

        assert_eq!(body["response_format"]["type"], "json_object");
    }

    #[test]
    fn asking_for_text_sets_no_response_format() {
        let provider = OpenAIProvider::new("key");
        let mut request = request();
        request.response_format = Some(ResponseFormat::Text);

        let body = provider.build_chat_request_body(&request);

        assert!(body.get("response_format").is_none());
    }

    #[test]
    fn an_image_body_carries_the_size_the_count_and_the_style() {
        let provider = OpenAIProvider::new("test-key");
        let request = ImageRequest {
            prompt: "A sunset over mountains".into(),
            negative_prompt: None,
            width: 1024,
            height: 1024,
            style: Some("vivid".into()),
            reference_images: Vec::new(),
            num_images: 1,
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
        let provider = OpenAIProvider::new("key");
        let request = ImageRequest::new("A cat", 512, 512);

        let body = provider.build_image_request_body(&request);

        assert!(body.get("style").is_none());
        assert_eq!(body["n"], 1);
        assert_eq!(body["size"], "512x512");
    }

    #[test]
    fn an_embedding_body_carries_every_text() {
        let provider = OpenAIProvider::new("test-key");

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
        let provider = OpenAIProvider::new("key");
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

        let response = OpenAIProvider::parse_chat_response(&body).unwrap();

        assert_eq!(response.content, "fn main() {}");
        assert_eq!(response.model, "gpt-4o-2024-05-13");
        assert_eq!(response.input_tokens, 30);
        assert_eq!(response.output_tokens, 20);
        assert_eq!(response.finish_reason, "stop");
    }

    #[test]
    fn a_response_without_content_is_an_error_not_an_empty_answer() {
        assert!(OpenAIProvider::parse_chat_response(&serde_json::json!({ "id": "x" })).is_err());
        assert!(
            OpenAIProvider::parse_chat_response(&serde_json::json!({ "choices": [] })).is_err()
        );
    }

    #[test]
    fn the_optional_parts_of_a_response_have_defaults() {
        let body = serde_json::json!({
            "choices": [{ "message": { "role": "assistant", "content": "hello" } }]
        });

        let response = OpenAIProvider::parse_chat_response(&body).unwrap();

        assert_eq!(response.model, "unknown");
        assert_eq!(response.input_tokens, 0);
        assert_eq!(response.output_tokens, 0);
        assert_eq!(response.finish_reason, "stop");
    }

    #[test]
    fn the_provider_reports_what_it_can_do() {
        let provider = OpenAIProvider::new("key");

        assert_eq!(TextProvider::name(&provider), "openai");
        assert_eq!(ImageProvider::name(&provider), "openai");
        assert_eq!(EmbeddingProvider::name(&provider), "openai");
        assert_eq!(TranscriptionProvider::name(&provider), "openai");
        assert!(provider.supports_structured_output());
        assert_eq!(provider.max_context_tokens(), MAX_CONTEXT_TOKENS);
        assert_eq!(provider.max_resolution(), MAX_RESOLUTION);
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

        let provider = OpenAIProvider::with_base_url("sk-test", DEFAULT_MODEL, server.uri());
        let response = provider
            .complete(&TextRequest::new("sys", "usr"))
            .await
            .unwrap();

        assert_eq!(response.content, "hello");
        assert_eq!(response.input_tokens, 7);
        assert_eq!(response.output_tokens, 3);
    }

    #[tokio::test]
    async fn a_refused_call_carries_the_status_and_the_body() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(429).set_body_string("rate limited"))
            .mount(&server)
            .await;

        let provider = OpenAIProvider::with_base_url("sk-test", DEFAULT_MODEL, server.uri());
        let error = provider
            .complete(&TextRequest::new("sys", "usr"))
            .await
            .unwrap_err();

        match error {
            ModalityError::ApiError { status, message } => {
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

        let provider = OpenAIProvider::with_base_url("sk-test", DEFAULT_MODEL, server.uri());
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

        let provider = OpenAIProvider::with_base_url("sk-test", DEFAULT_MODEL, server.uri());
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

        let provider = OpenAIProvider::with_base_url("sk-test", DEFAULT_MODEL, server.uri());
        let response = provider.transcribe(audio.path()).await.unwrap();

        assert_eq!(response.text, "Hello world");
        assert_eq!(response.language, "en");
        assert_eq!(response.segments.len(), 1);
        assert!((response.segments[0].confidence + 0.25).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn a_missing_audio_file_is_an_io_error() {
        let provider = OpenAIProvider::new("key");
        let error = provider
            .transcribe(Path::new("/nonexistent/audio.mp3"))
            .await
            .unwrap_err();
        assert!(matches!(error, ModalityError::IoError(_)), "{error:?}");
    }

    #[tokio::test]
    async fn the_unimplemented_paths_say_so_rather_than_failing_obscurely() {
        let provider = OpenAIProvider::new("key");

        let streaming = provider
            .stream_complete(&TextRequest::new("sys", "usr"))
            .await
            .err()
            .unwrap();
        assert!(matches!(streaming, ModalityError::Unsupported(_)));

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
        assert!(matches!(edit, ModalityError::Unsupported(_)));

        let variations = provider.variations(&[], 1).await.err().unwrap();
        assert!(matches!(variations, ModalityError::Unsupported(_)));
    }
}
