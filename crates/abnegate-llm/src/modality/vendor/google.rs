use abnegate_secret::SecretValue;
use async_trait::async_trait;
use futures::Stream;

use crate::modality::{ResponseFormat, TextProvider, TextRequest, TextResponse};
use crate::provider::ProviderError;

const BASE_URL: &str = "https://generativelanguage.googleapis.com/v1beta/models";
const DEFAULT_MODEL: &str = "gemini-2.5-pro";
const MAX_CONTEXT_TOKENS: u32 = 1_000_000;

pub struct GeminiProvider {
    api_key: SecretValue,
    model: String,
    base_url: String,
    client: reqwest::Client,
}

impl GeminiProvider {
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

    pub fn build_request_body(&self, request: &TextRequest) -> serde_json::Value {
        let mut body = serde_json::json!({
            "contents": [
                { "parts": [{ "text": request.user_prompt }] }
            ],
            "generationConfig": {
                "temperature": request.temperature,
                "maxOutputTokens": request.max_tokens
            }
        });

        if !request.system_prompt.is_empty()
            && let Some(object) = body.as_object_mut()
        {
            object.insert(
                "systemInstruction".into(),
                serde_json::json!({ "parts": [{ "text": request.system_prompt }] }),
            );
        }

        body
    }

    /// The endpoint for this model, with the key as a query parameter.
    ///
    /// The key is percent-encoded rather than interpolated, so a key holding a
    /// reserved character cannot change the request's shape.
    pub fn build_request_url(&self) -> String {
        let endpoint = format!("{}/{}:generateContent", self.base_url, self.model);
        let mut url = match url::Url::parse(&endpoint) {
            Ok(url) => url,
            Err(_) => return endpoint,
        };
        url.query_pairs_mut()
            .append_pair("key", self.api_key.expose());
        url.to_string()
    }

    pub fn parse_response(body: &serde_json::Value) -> Result<TextResponse, ProviderError> {
        let candidate = body
            .get("candidates")
            .and_then(|candidates| candidates.as_array())
            .and_then(|candidates| candidates.first());

        let content = candidate
            .and_then(|candidate| candidate.get("content"))
            .and_then(|content| content.get("parts"))
            .and_then(|parts| parts.as_array())
            .and_then(|parts| parts.first())
            .and_then(|part| part.get("text"))
            .and_then(|text| text.as_str())
            .ok_or_else(|| {
                ProviderError::parse("missing candidates[0].content.parts[0].text in response")
            })?
            .to_string();

        Ok(TextResponse {
            content,
            model: body
                .get("modelVersion")
                .and_then(|model| model.as_str())
                .unwrap_or("unknown")
                .to_string(),
            input_tokens: usage(body, "promptTokenCount"),
            output_tokens: usage(body, "candidatesTokenCount"),
            finish_reason: candidate
                .and_then(|candidate| candidate.get("finishReason"))
                .and_then(|reason| reason.as_str())
                .unwrap_or("STOP")
                .to_string(),
        })
    }

    async fn send(&self, body: &serde_json::Value) -> Result<serde_json::Value, ProviderError> {
        let response = self
            .client
            .post(self.build_request_url())
            .header("Content-Type", "application/json")
            .json(body)
            .send()
            .await
            .map_err(|error| ProviderError::network(error.without_url()))?;

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
            .map_err(|error| ProviderError::parse(error.without_url()))
    }
}

fn usage(body: &serde_json::Value, field: &str) -> u32 {
    body.get("usageMetadata")
        .and_then(|usage| usage.get(field))
        .and_then(|count| count.as_u64())
        .unwrap_or(0) as u32
}

#[async_trait]
impl TextProvider for GeminiProvider {
    fn name(&self) -> &str {
        "google"
    }

    fn supports_structured_output(&self) -> bool {
        true
    }

    fn max_context_tokens(&self) -> u32 {
        MAX_CONTEXT_TOKENS
    }

    async fn complete(&self, request: &TextRequest) -> Result<TextResponse, ProviderError> {
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
            return parse_content(&response.content);
        };

        let mut body = self.build_request_body(request);
        if let Some(config) = body
            .get_mut("generationConfig")
            .and_then(|config| config.as_object_mut())
        {
            config.insert(
                "responseMimeType".into(),
                serde_json::Value::String("application/json".into()),
            );
            config.insert("responseSchema".into(), schema.clone());
        }

        let json = self.send(&body).await?;
        parse_content(&Self::parse_response(&json)?.content)
    }

    async fn stream_complete(
        &self,
        _request: &TextRequest,
    ) -> Result<Box<dyn Stream<Item = Result<String, ProviderError>> + Send + Unpin>, ProviderError>
    {
        Err(ProviderError::unsupported(
            "streaming not yet implemented for Gemini provider",
        ))
    }
}

fn parse_content(content: &str) -> Result<serde_json::Value, ProviderError> {
    serde_json::from_str(content).map_err(|error| {
        ProviderError::parse(format!("failed to parse structured output: {error}"))
    })
}

#[cfg(test)]
mod tests {
    use wiremock::matchers::{method, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    #[test]
    fn a_body_carries_the_prompt_the_system_instruction_and_the_settings() {
        let provider = GeminiProvider::new("test-key");
        let request = TextRequest {
            system_prompt: "You are a designer.".into(),
            user_prompt: "Design a puzzle mechanic.".into(),
            temperature: 0.8,
            max_tokens: 4096,
            response_format: None,
            context: None,
        };

        let body = provider.build_request_body(&request);

        let contents = body["contents"].as_array().unwrap();
        assert_eq!(contents.len(), 1);
        assert_eq!(contents[0]["parts"][0]["text"], "Design a puzzle mechanic.");
        assert_eq!(
            body["systemInstruction"]["parts"][0]["text"],
            "You are a designer."
        );
        assert_eq!(body["generationConfig"]["temperature"], 0.8);
        assert_eq!(body["generationConfig"]["maxOutputTokens"], 4096);
    }

    #[test]
    fn an_empty_system_prompt_sends_no_system_instruction() {
        let provider = GeminiProvider::new("key");
        let request = TextRequest::new("", "Hello");

        let body = provider.build_request_body(&request);

        assert!(body.get("systemInstruction").is_none());
    }

    #[test]
    fn the_url_names_the_model_and_carries_the_key() {
        let url = GeminiProvider::new("my-api-key").build_request_url();

        assert!(url.starts_with(BASE_URL), "{url}");
        assert!(url.contains(DEFAULT_MODEL), "{url}");
        assert!(url.contains(":generateContent"), "{url}");
        assert!(url.contains("key=my-api-key"), "{url}");
    }

    #[test]
    fn a_custom_model_reaches_the_url() {
        let url = GeminiProvider::with_model("key", "gemini-2.5-flash").build_request_url();

        assert!(url.contains("gemini-2.5-flash"), "{url}");
        assert!(!url.contains(DEFAULT_MODEL), "{url}");
    }

    #[test]
    fn a_key_holding_a_reserved_character_is_encoded_not_interpolated() {
        let url = GeminiProvider::new("a&b=c").build_request_url();

        assert!(url.contains("key=a%26b%3Dc"), "{url}");
    }

    #[test]
    fn a_response_is_read_in_full() {
        let body = serde_json::json!({
            "candidates": [{
                "content": { "parts": [{ "text": "Here is an idea..." }], "role": "model" },
                "finishReason": "STOP"
            }],
            "usageMetadata": {
                "promptTokenCount": 20,
                "candidatesTokenCount": 50,
                "totalTokenCount": 70
            },
            "modelVersion": "gemini-2.5-pro"
        });

        let response = GeminiProvider::parse_response(&body).unwrap();

        assert_eq!(response.content, "Here is an idea...");
        assert_eq!(response.model, "gemini-2.5-pro");
        assert_eq!(response.input_tokens, 20);
        assert_eq!(response.output_tokens, 50);
        assert_eq!(response.finish_reason, "STOP");
    }

    #[test]
    fn a_response_without_content_is_an_error_not_an_empty_answer() {
        assert!(
            GeminiProvider::parse_response(&serde_json::json!({ "modelVersion": "x" })).is_err()
        );
        assert!(GeminiProvider::parse_response(&serde_json::json!({ "candidates": [] })).is_err());
    }

    #[test]
    fn the_optional_parts_of_a_response_have_defaults() {
        let body = serde_json::json!({
            "candidates": [{
                "content": { "parts": [{ "text": "data" }] },
                "finishReason": "MAX_TOKENS"
            }]
        });

        let response = GeminiProvider::parse_response(&body).unwrap();

        assert_eq!(response.model, "unknown");
        assert_eq!(response.input_tokens, 0);
        assert_eq!(response.output_tokens, 0);
        assert_eq!(response.finish_reason, "MAX_TOKENS");
    }

    #[test]
    fn the_provider_reports_what_it_can_do() {
        let provider = GeminiProvider::new("key");
        assert_eq!(provider.name(), "google");
        assert!(provider.supports_structured_output());
        assert_eq!(provider.max_context_tokens(), MAX_CONTEXT_TOKENS);
    }

    #[tokio::test]
    async fn a_completion_is_sent_with_the_key_and_read_back() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(query_param("key", "sk-test"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "candidates": [{
                    "content": { "parts": [{ "text": "hello" }] },
                    "finishReason": "STOP"
                }],
                "usageMetadata": { "promptTokenCount": 2, "candidatesTokenCount": 1 },
                "modelVersion": DEFAULT_MODEL
            })))
            .mount(&server)
            .await;

        let provider = GeminiProvider::with_base_url("sk-test", DEFAULT_MODEL, server.uri());
        let response = provider
            .complete(&TextRequest::new("sys", "usr"))
            .await
            .unwrap();

        assert_eq!(response.content, "hello");
        assert_eq!(response.input_tokens, 2);
        assert_eq!(response.output_tokens, 1);
    }

    #[tokio::test]
    async fn a_structured_answer_is_parsed_out_of_the_text_part() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "candidates": [{
                    "content": { "parts": [{ "text": r#"{"beats": 3}"# }] },
                    "finishReason": "STOP"
                }]
            })))
            .mount(&server)
            .await;

        let provider = GeminiProvider::with_base_url("sk-test", DEFAULT_MODEL, server.uri());
        let mut request = TextRequest::new("sys", "usr");
        request.response_format = Some(ResponseFormat::Json {
            schema: Some(serde_json::json!({ "type": "object" })),
        });

        let value = provider.complete_structured(&request).await.unwrap();

        assert_eq!(value["beats"], 3);
    }

    #[tokio::test]
    async fn a_refused_call_carries_the_status_and_the_body() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(400).set_body_string("bad request"))
            .mount(&server)
            .await;

        let provider = GeminiProvider::with_base_url("sk-test", DEFAULT_MODEL, server.uri());
        let error = provider
            .complete(&TextRequest::new("sys", "usr"))
            .await
            .unwrap_err();

        match error {
            ProviderError::Api { status, message } => {
                assert_eq!(status, 400);
                assert_eq!(message, "bad request");
            }
            other => panic!("expected ApiError, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn streaming_says_it_is_not_implemented() {
        let error = GeminiProvider::new("key")
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
