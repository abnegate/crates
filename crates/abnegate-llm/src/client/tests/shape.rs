use futures::StreamExt;
use serde_json::Value;
use serde_json::json;
use wiremock::Match;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::Request;
use wiremock::ResponseTemplate;
use wiremock::matchers::body_partial_json;
use wiremock::matchers::method;
use wiremock::matchers::path;

use crate::client::LlmClient;
use crate::client::LlmConfig;
use crate::client::RequestOptions;
use crate::dialect::Dialect;
use crate::dialect::TEMPLATE_STOPS;
use crate::modality::ResponseFormat;
use crate::provider::CompletionRequest;
use crate::reasoning::Effort;
use crate::wire::ChatRequest;
use crate::wire::Message;
use crate::wire::ToolCall;
use crate::wire::ToolDefinition;

const TEMPERATURE: f32 = 1.4;
const RESERVED: u32 = 321;
const UPLOAD: &str = "data:image/png;base64,dXBsb2Fk";
const SCREENSHOT: &str = "data:image/png;base64,c2NyZWVu";
const GENERATED: &str = "data:image/png;base64,Z2VuZXJhdGVk";
const DRAWN: &str = "data:image/png;base64,ZHJhd24=";

struct Without(&'static str);

impl Match for Without {
    fn matches(&self, request: &Request) -> bool {
        serde_json::from_slice::<Value>(&request.body).is_ok_and(|body| body.get(self.0).is_none())
    }
}

fn completion() -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "id": "completion",
        "object": "chat.completion",
        "created": 0,
        "model": "test",
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": "hi"},
            "finish_reason": "stop"
        }]
    }))
}

fn streamed() -> ResponseTemplate {
    ResponseTemplate::new(200)
        .insert_header("content-type", "text/event-stream")
        .set_body_string(
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hi\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n",
        )
}

fn stops() -> Vec<String> {
    TEMPLATE_STOPS
        .iter()
        .map(|stop| (*stop).to_string())
        .chain(["User:", " ", "Human:", "###", "END", "STOP"].map(String::from))
        .collect()
}

fn client(server: &MockServer, dialect: Dialect) -> LlmClient {
    LlmClient::new(
        LlmConfig::new(server.uri(), "model", "")
            .with_temperature(TEMPERATURE)
            .with_maximum_tokens(RESERVED)
            .with_dialect(dialect),
    )
    .with_stop(stops())
    .with_ollama_context("model", 8192)
}

async fn last_body(server: &MockServer) -> Value {
    let requests = server.received_requests().await.unwrap_or_default();
    let request = requests.last().expect("a request reached the endpoint");
    serde_json::from_slice(&request.body).expect("a JSON body")
}

async fn sent(server: &MockServer, dialect: Dialect, model: &str) -> Value {
    let answered = client(server, dialect)
        .chat_with_model(model, &[Message::user("hi")], None)
        .await;
    assert!(answered.is_ok(), "{dialect:?} {model}: {answered:?}");
    last_body(server).await
}

async fn drained(
    client: &LlmClient,
    model: &str,
    messages: &[Message],
    tools: Option<&[ToolDefinition]>,
) {
    let chunks: Vec<_> = client
        .chat_stream_with_model(model, messages, tools)
        .await
        .expect("the stream opens")
        .collect()
        .await;
    assert!(chunks.iter().all(Result::is_ok), "{chunks:?}");
}

fn stop_count(body: &Value) -> usize {
    body.get("stop")
        .and_then(Value::as_array)
        .map_or(0, Vec::len)
}

#[tokio::test]
async fn openai_is_sent_at_most_four_stops_and_no_template_tokens() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(body_partial_json(json!({
            "max_tokens": RESERVED,
            "stop": ["User:", " ", "Human:", "###"],
        })))
        .and(Without("max_completion_tokens"))
        .and(Without("num_ctx"))
        .respond_with(completion())
        .expect(1)
        .mount(&server)
        .await;

    let body = sent(&server, Dialect::OpenAI, "gpt-4o").await;

    assert_eq!(stop_count(&body), 4, "{body}");
    assert!(body.get("temperature").is_some(), "{body}");
    server.verify().await;
}

#[tokio::test]
async fn an_openai_reasoning_model_is_sent_no_stop_no_temperature_and_a_completion_budget() {
    for model in ["o3-mini", "o1", "o4-mini", "gpt-5"] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .and(body_partial_json(
                json!({ "max_completion_tokens": RESERVED }),
            ))
            .and(Without("max_tokens"))
            .and(Without("temperature"))
            .and(Without("stop"))
            .respond_with(completion())
            .expect(1)
            .mount(&server)
            .await;

        let body = sent(&server, Dialect::OpenAI, model).await;

        assert_eq!(stop_count(&body), 0, "{model}: {body}");
        server.verify().await;
    }
}

#[tokio::test]
async fn an_openai_chat_variant_keeps_its_temperature_and_stops() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(body_partial_json(json!({ "max_tokens": RESERVED })))
        .and(Without("max_completion_tokens"))
        .respond_with(completion())
        .expect(1)
        .mount(&server)
        .await;

    let body = sent(&server, Dialect::OpenAI, "gpt-5-chat-latest").await;

    assert_eq!(stop_count(&body), 4, "{body}");
    assert_eq!(
        body["temperature"].as_f64().map(|value| value as f32),
        Some(TEMPERATURE),
        "{body}"
    );
    server.verify().await;
}

#[tokio::test]
async fn an_openai_reasoning_model_streams_under_the_same_shape() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(body_partial_json(json!({
            "max_completion_tokens": RESERVED,
            "stream": true,
        })))
        .and(Without("max_tokens"))
        .and(Without("temperature"))
        .and(Without("stop"))
        .respond_with(streamed())
        .expect(1)
        .mount(&server)
        .await;

    drained(
        &client(&server, Dialect::OpenAI),
        "o3",
        &[Message::user("hi")],
        None,
    )
    .await;
    server.verify().await;
}

#[tokio::test]
async fn openai_is_sent_reasoning_effort_only_for_a_model_that_reasons() {
    for (model, expected) in [("o3", Some("high")), ("gpt-4o", None)] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(completion())
            .mount(&server)
            .await;
        let answered = client(&server, Dialect::OpenAI)
            .with_reasoning(model, Effort::High)
            .chat_with_model(model, &[Message::user("hi")], None)
            .await;
        assert!(answered.is_ok(), "{answered:?}");
        let body = last_body(&server).await;

        assert_eq!(
            body.get("reasoning_effort").and_then(Value::as_str),
            expected,
            "{model}: {body}"
        );
    }
}

#[tokio::test]
async fn anthropic_is_sent_every_non_blank_stop_and_a_temperature_of_at_most_one() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(body_partial_json(json!({
            "max_tokens": RESERVED,
            "temperature": 1.0,
        })))
        .and(Without("max_completion_tokens"))
        .and(Without("num_ctx"))
        .and(Without("reasoning_effort"))
        .respond_with(completion())
        .expect(1)
        .mount(&server)
        .await;

    let answered = client(&server, Dialect::Anthropic)
        .with_reasoning("claude-sonnet-4-5", Effort::High)
        .chat_with_model("claude-sonnet-4-5", &[Message::user("hi")], None)
        .await;
    assert!(answered.is_ok(), "{answered:?}");
    let body = last_body(&server).await;

    assert_eq!(stop_count(&body), stops().len() - 1, "{body}");
    assert!(
        body["stop"]
            .as_array()
            .is_some_and(|stops| stops.iter().all(|stop| stop != " ")),
        "{body}"
    );
    server.verify().await;
}

fn call(id: &str, name: &str, arguments: &str) -> ToolCall {
    ToolCall::function(id, name, arguments)
}

fn history() -> Vec<Message> {
    let mut asked = Message::user("What is on screen?");
    asked.images = vec![UPLOAD.to_string()];
    let mut requested = Message::assistant_with_tools(vec![
        call("call_1", "screenshot", ""),
        call("call_2", "list", r#"{"path":"."}"#),
    ]);
    requested.reasoning_content = Some("I should look first".to_string());
    requested.thinking_blocks =
        vec![json!({ "type": "thinking", "thinking": "look", "signature": "c2ln" })];
    let mut captured = Message::tool_result("call_1", "captured");
    captured.images = vec![SCREENSHOT.to_string()];
    let listed = Message::tool_result("call_2", "a.rs");
    let mut answered = Message::assistant("Here it is");
    answered.images = vec![GENERATED.to_string()];
    answered.reasoning_content = Some("the screen shows a.rs".to_string());
    let mut drawn = Message::assistant("");
    drawn.images = vec![DRAWN.to_string()];
    vec![
        Message::system("Be brief."),
        asked,
        requested,
        captured,
        listed,
        answered,
        drawn,
        Message::user("Again."),
    ]
}

fn tools() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition::function("screenshot", "Capture the screen", json!({})),
        ToolDefinition::function(
            "list",
            "List a directory",
            json!({ "type": "object", "properties": { "path": { "type": "string" } } }),
        ),
    ]
}

fn image(url: &str) -> Value {
    json!({ "type": "image_url", "image_url": { "url": url } })
}

fn forwarded(label: &str, url: &str) -> Value {
    json!({ "role": "user", "content": [{ "type": "text", "text": label }, image(url)] })
}

fn provider_contract() -> (Value, Value) {
    let messages = json!([
        { "role": "system", "content": "Be brief." },
        {
            "role": "user",
            "content": [{ "type": "text", "text": "What is on screen?" }, image(UPLOAD)],
        },
        {
            "role": "assistant",
            "tool_calls": [
                {
                    "id": "call_1",
                    "type": "function",
                    "function": { "name": "screenshot", "arguments": "{}" },
                },
                {
                    "id": "call_2",
                    "type": "function",
                    "function": { "name": "list", "arguments": r#"{"path":"."}"# },
                },
            ],
        },
        { "role": "tool", "content": "captured", "tool_call_id": "call_1" },
        { "role": "tool", "content": "a.rs", "tool_call_id": "call_2" },
        forwarded("[Image from the tool result for call_1]", SCREENSHOT),
        { "role": "assistant", "content": "Here it is" },
        forwarded("[Image from the previous assistant message]", GENERATED),
        forwarded("[Image from the previous assistant message]", DRAWN),
        { "role": "user", "content": "Again." },
    ]);
    let tools = json!([
        {
            "type": "function",
            "function": {
                "name": "screenshot",
                "description": "Capture the screen",
                "parameters": { "type": "object", "properties": {} },
            },
        },
        {
            "type": "function",
            "function": {
                "name": "list",
                "description": "List a directory",
                "parameters": {
                    "type": "object",
                    "properties": { "path": { "type": "string" } },
                },
            },
        },
    ]);
    (messages, tools)
}

async fn contract(dialect: Dialect, stream: bool) -> Value {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(if stream { streamed() } else { completion() })
        .expect(1)
        .mount(&server)
        .await;
    let client = client(&server, dialect);
    let (messages, tools) = (history(), tools());
    if stream {
        drained(&client, "model", &messages, Some(&tools)).await;
    } else {
        let answered = client
            .chat_with_model("model", &messages, Some(&tools))
            .await;
        assert!(answered.is_ok(), "{dialect:?}: {answered:?}");
    }
    server.verify().await;
    last_body(&server).await
}

#[tokio::test]
async fn a_provider_api_is_sent_only_the_message_shapes_it_accepts() {
    let (messages, tools) = provider_contract();
    for dialect in [Dialect::OpenAI, Dialect::Anthropic] {
        for stream in [false, true] {
            let body = contract(dialect, stream).await;

            assert_eq!(body["messages"], messages, "{dialect:?} stream={stream}");
            assert_eq!(body["tools"], tools, "{dialect:?} stream={stream}");
            assert_eq!(body["stream"], json!(stream), "{dialect:?}");
            assert!(body.get("tool_choice").is_none(), "{dialect:?}: {body}");
        }
    }
}

#[tokio::test]
async fn a_provider_api_is_never_sent_an_empty_tool_list() {
    for dialect in [Dialect::OpenAI, Dialect::Anthropic] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(Without("tools"))
            .respond_with(completion())
            .expect(1)
            .mount(&server)
            .await;

        let answered = client(&server, dialect)
            .chat_with_model("model", &[Message::user("hi")], Some(&[]))
            .await;

        assert!(answered.is_ok(), "{dialect:?}: {answered:?}");
        server.verify().await;
    }
}

#[tokio::test]
async fn a_compatible_endpoint_is_sent_the_history_as_it_always_was() {
    let history = history();
    let tools = tools();
    for stream in [false, true] {
        let body = contract(Dialect::Compatible, stream).await;

        assert_eq!(
            body["messages"],
            serde_json::to_value(&history).expect("a history"),
            "stream={stream}"
        );
        assert_eq!(
            body["tools"],
            serde_json::to_value(&tools).expect("tools"),
            "stream={stream}"
        );
        assert_eq!(
            body["messages"][3]["content"][1],
            image(SCREENSHOT),
            "a gateway translates a tool result's image itself"
        );
    }
}

#[tokio::test]
async fn a_compatible_endpoint_is_sent_the_request_as_it_always_was() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(body_partial_json(json!({
            "max_tokens": RESERVED,
            "num_ctx": 8192,
        })))
        .and(Without("max_completion_tokens"))
        .respond_with(completion())
        .expect(1)
        .mount(&server)
        .await;

    let body = sent(&server, Dialect::Compatible, "model").await;

    assert_eq!(stop_count(&body), stops().len(), "{body}");
    assert_eq!(
        body["temperature"].as_f64().map(|value| value as f32),
        Some(TEMPERATURE),
        "{body}"
    );
    server.verify().await;
}

#[tokio::test]
async fn a_provider_request_is_shaped_like_any_other() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(body_partial_json(
            json!({ "max_completion_tokens": RESERVED }),
        ))
        .and(Without("max_tokens"))
        .and(Without("temperature"))
        .and(Without("stop"))
        .respond_with(completion())
        .expect(1)
        .mount(&server)
        .await;
    let messages = [Message::user("hi")];
    let request = CompletionRequest::new("o3", &messages, RequestOptions::new(RESERVED))
        .with_temperature(0.2);

    let answered = client(&server, Dialect::OpenAI)
        .chat_with_request(request)
        .await;

    assert!(answered.is_ok(), "{answered:?}");
    server.verify().await;
}

#[test]
fn a_compatible_endpoint_is_sent_both_budget_fields_as_the_caller_set_them() {
    let messages = [Message::user("hi")];
    let client = LlmClient::new(LlmConfig::default());

    let both = client
        .body(
            ChatRequest::new("o3", &messages)
                .with_maximum_tokens(10)
                .with_maximum_completion_tokens(20),
        )
        .expect("a body");
    let completion = client
        .body(ChatRequest::new("o3", &messages).with_maximum_completion_tokens(20))
        .expect("a body");

    assert_eq!(both["max_tokens"], 10, "{both}");
    assert_eq!(both["max_completion_tokens"], 20, "{both}");
    assert!(completion.get("max_tokens").is_none(), "{completion}");
    assert_eq!(completion["max_completion_tokens"], 20, "{completion}");
}

#[test]
fn a_provider_api_is_sent_the_budget_in_the_one_field_its_model_reads() {
    let messages = [Message::user("hi")];
    let client = LlmClient::new(LlmConfig::default().with_dialect(Dialect::OpenAI));

    let reasoning = client
        .body(ChatRequest::new("o3", &messages).with_maximum_tokens(10))
        .expect("a body");
    let conversational = client
        .body(ChatRequest::new("gpt-4o", &messages).with_maximum_completion_tokens(20))
        .expect("a body");

    assert_eq!(reasoning["max_completion_tokens"], 10, "{reasoning}");
    assert!(reasoning.get("max_tokens").is_none(), "{reasoning}");
    assert_eq!(conversational["max_tokens"], 20, "{conversational}");
    assert!(
        conversational.get("max_completion_tokens").is_none(),
        "{conversational}"
    );
}

#[test]
fn a_compatible_endpoint_is_sent_an_empty_stop_list_as_given() {
    let messages = [Message::user("hi")];
    let client = LlmClient::new(LlmConfig::default());

    let body = client
        .body(ChatRequest::new("model", &messages).with_stop(&[]))
        .expect("a body");

    assert_eq!(body["stop"], json!([]), "{body}");
}

#[test]
fn anthropic_is_sent_only_the_strict_schema_response_format_it_accepts() {
    let messages = [Message::user("hi")];
    let client = LlmClient::new(LlmConfig::default().with_dialect(Dialect::Anthropic));
    let guidance = json!({"type": "object", "properties": {"answer": {"type": "string"}}});
    let strict = json!({
        "type": "object",
        "properties": {"answer": {"type": "string"}},
        "required": ["answer"],
        "additionalProperties": false
    });

    let body = client
        .body(
            ChatRequest::new("claude-sonnet-4-6", &messages)
                .with_response_format(&ResponseFormat::strict_json_schema(strict.clone())),
        )
        .expect("a body");
    assert_eq!(body["response_format"]["type"], "json_schema", "{body}");
    assert_eq!(
        body["response_format"]["json_schema"]["strict"], true,
        "{body}"
    );
    assert_eq!(
        body["response_format"]["json_schema"]["schema"], strict,
        "{body}"
    );

    for format in [
        ResponseFormat::json_schema(guidance),
        ResponseFormat::json(),
        ResponseFormat::Text,
    ] {
        let body = client
            .body(ChatRequest::new("claude-sonnet-4-6", &messages).with_response_format(&format))
            .expect("a body");
        assert!(body.get("response_format").is_none(), "{format:?}: {body}");
    }

    let compatible = LlmClient::new(LlmConfig::default())
        .body(ChatRequest::new("model", &messages).with_response_format(&ResponseFormat::json()))
        .expect("a body");
    assert_eq!(
        compatible["response_format"]["type"], "json_object",
        "{compatible}"
    );
}
