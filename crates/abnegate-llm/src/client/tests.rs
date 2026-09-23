use std::time::Duration;
use std::time::Instant;

use futures::StreamExt;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;
use wiremock::matchers::header;
use wiremock::matchers::header_exists;
use wiremock::matchers::method;

use super::LlmClient;
use super::LlmConfig;
use crate::error::LlmError;
use crate::reasoning::Effort;
use crate::wire::ChatRequest;
use crate::wire::Message;

const CHUNK: &str = r#"{"choices":[{"index":0,"delta":{"content":"hi"},"finish_reason":null}]}"#;
const COMPLETION: &str = r#"{"id":"c","object":"chat.completion","created":0,"model":"m","choices":[{"index":0,"message":{"role":"assistant","content":"hi"},"finish_reason":"stop"}]}"#;

fn client_for(base_url: &str) -> LlmClient {
    LlmClient::new(LlmConfig::new(base_url, "gpt-4", ""))
}

fn request<'a>(model: &'a str, messages: &'a [Message]) -> ChatRequest<'a> {
    ChatRequest {
        model,
        messages,
        tools: None,
        tool_choice: None,
        temperature: None,
        max_tokens: Some(4096),
        stream: None,
        stop: None,
        response_format: None,
    }
}

async fn streamed(body: &str) -> Vec<Result<crate::wire::ChatStreamChunk, LlmError>> {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(body),
        )
        .mount(&server)
        .await;

    client_for(&server.uri())
        .chat_stream(&[Message::user("hi")], None)
        .await
        .expect("the stream opens")
        .collect()
        .await
}

fn content(result: &Result<crate::wire::ChatStreamChunk, LlmError>) -> Option<&str> {
    result
        .as_ref()
        .ok()?
        .choices
        .first()?
        .delta
        .content
        .as_deref()
}

#[test]
fn a_client_exposes_the_config_it_was_built_from() {
    let config = LlmConfig::new(
        "https://custom.openai.com/v1",
        "gpt-4-turbo",
        "sk-custom-key",
    )
    .with_temperature(0.3)
    .with_max_tokens(8192);
    let client = LlmClient::new(config.clone());

    assert_eq!(client.config().base_url, config.base_url);
    assert_eq!(client.config().api_key.expose(), "sk-custom-key");
    assert_eq!(client.config().default_model, "gpt-4-turbo");
    assert!((client.config().temperature - 0.3).abs() < f32::EPSILON);
    assert_eq!(client.config().max_tokens, 8192);
}

#[test]
fn a_client_never_prints_its_key() {
    const KEY: &str = "sk-test-6b02da97e15c4f38";
    let client = LlmClient::new(LlmConfig::new("https://api.openai.com/v1", "gpt-4", KEY));
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

    let enabled = client.body(request("thinker", &messages)).unwrap();
    assert_eq!(enabled["reasoning_effort"], "high");

    let other = client.body(request("other", &messages)).unwrap();
    assert!(other.get("reasoning_effort").is_none());

    let compact = client
        .without_reasoning()
        .body(request("thinker", &messages))
        .unwrap();
    assert!(compact.get("reasoning_effort").is_none());
}

#[tokio::test]
async fn a_key_is_sent_as_a_bearer_token_and_an_empty_one_not_at_all() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(header("authorization", "Bearer sk-configured"))
        .respond_with(ResponseTemplate::new(200).set_body_string(COMPLETION))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(header_exists("authorization"))
        .respond_with(ResponseTemplate::new(401))
        .expect(0)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_string(COMPLETION))
        .expect(1)
        .mount(&server)
        .await;

    LlmClient::new(LlmConfig::new(server.uri(), "m", "sk-configured"))
        .chat(&[Message::user("hi")], None)
        .await
        .expect("the keyed request succeeds");
    client_for(&server.uri())
        .chat(&[Message::user("hi")], None)
        .await
        .expect("the keyless request succeeds");
}

#[tokio::test]
async fn a_rejection_body_echoing_the_key_is_redacted() {
    const ECHOED: &str = "sk-proj-0b1c2d3e4f5a6b7c8d9e0f1a2b3c4d5e";
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(401).set_body_string(format!(
            r#"{{"error":{{"message":"Incorrect API key provided: {ECHOED}"}}}}"#
        )))
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

#[tokio::test]
async fn a_completion_that_outlives_its_deadline_fails_at_the_deadline() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(COMPLETION)
                .set_delay(Duration::from_secs(5)),
        )
        .mount(&server)
        .await;
    let client = LlmClient::new(
        LlmConfig::new(server.uri(), "m", "").with_timeout(Duration::from_millis(200)),
    );

    let started = Instant::now();
    let error = client
        .chat(&[Message::user("hi")], None)
        .await
        .expect_err("the endpoint is too slow");

    assert!(matches!(error, LlmError::Timeout(_)), "{error:?}");
    assert!(started.elapsed() < Duration::from_secs(3));
}

#[tokio::test]
async fn a_stream_that_stalls_fails_after_the_read_timeout() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = vec![0_u8; 8192];
        let _ = socket.read(&mut request).await.unwrap();
        let frame = format!("data: {CHUNK}\n\n");
        socket
            .write_all(
                format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\n\r\n{:x}\r\n{frame}\r\n",
                    frame.len()
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        std::future::pending::<()>().await;
    });
    let client = LlmClient::new(
        LlmConfig::new(format!("http://{address}"), "m", "")
            .with_read_timeout(Duration::from_millis(300)),
    );

    let started = Instant::now();
    let chunks: Vec<_> = tokio::time::timeout(
        Duration::from_secs(10),
        client
            .chat_stream(&[Message::user("hi")], None)
            .await
            .expect("the stream opens")
            .collect(),
    )
    .await
    .expect("the stream gave up on its own");
    server.abort();

    assert_eq!(chunks.len(), 2, "{chunks:?}");
    assert_eq!(content(&chunks[0]), Some("hi"));
    assert!(matches!(chunks[1], Err(LlmError::Timeout(_))), "{chunks:?}");
    assert!(started.elapsed() < Duration::from_secs(3));
}

#[tokio::test]
async fn a_stream_reads_data_lines_without_a_space_and_a_final_unterminated_frame() {
    let chunks = streamed(&format!("data:{CHUNK}\n\ndata: {CHUNK}")).await;

    assert_eq!(chunks.len(), 2, "{chunks:?}");
    assert!(chunks.iter().all(|chunk| content(chunk) == Some("hi")));
}

#[tokio::test]
async fn an_event_split_over_several_data_lines_is_one_chunk() {
    let chunks = streamed(concat!(
        "data: {\"choices\":[{\"index\":0,\n",
        "data: \"delta\":{\"content\":\"hi\"}}]}\n\n",
        "data: [DONE]\n\n"
    ))
    .await;

    assert_eq!(chunks.len(), 1, "{chunks:?}");
    assert_eq!(content(&chunks[0]), Some("hi"));
}

#[tokio::test]
async fn a_success_that_is_not_an_event_stream_is_a_failure() {
    let chunks = streamed(COMPLETION).await;

    assert_eq!(chunks.len(), 1, "{chunks:?}");
    assert!(
        matches!(&chunks[0], Err(LlmError::Stream(message)) if message.contains("without a single event")),
        "{chunks:?}"
    );
}

#[tokio::test]
async fn an_error_frame_mid_stream_ends_it_with_a_failure() {
    let chunks = streamed(&format!(
        "data: {CHUNK}\n\ndata: {{\"error\":{{\"message\":\"upstream overloaded\"}}}}\n\ndata: {CHUNK}\n\n"
    ))
    .await;

    assert_eq!(chunks.len(), 2, "{chunks:?}");
    assert_eq!(content(&chunks[0]), Some("hi"));
    assert!(
        matches!(&chunks[1], Err(LlmError::Stream(message)) if message.contains("upstream overloaded")),
        "{chunks:?}"
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
