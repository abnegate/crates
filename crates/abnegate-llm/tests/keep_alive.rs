use abnegate_llm::{LlmClient, LlmConfig, LlmError, Message};
use futures::StreamExt;
use serde_json::json;
use wiremock::matchers::{body_partial_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const RACERS: usize = 8;
const TURNS: usize = 12;

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

/// One completion on its own runtime, as a chat turn gets in a server.
fn turn(base_url: &str) {
    runtime().block_on(async {
        LlmClient::new(LlmConfig::new(base_url, "test", ""))
            .chat(&[Message::user("hi")], None)
            .await
            .expect("a turn reaches the server")
    });
}

/// Completions share one process-wide connection pool, but every pooled
/// connection is driven by a task belonging to the runtime that opened it.
/// Once that runtime is gone the pool can still hand the connection out, and
/// the send fails with "runtime dropped the dispatch task" without ever
/// reaching the server.
#[test]
fn a_pooled_connection_outliving_its_runtime_does_not_fail_the_next_turn() {
    let host = runtime();
    let server = host.block_on(async {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "completion",
                "object": "chat.completion",
                "created": 0,
                "model": "test",
                "choices": [{
                    "index": 0,
                    "message": {"role": "assistant", "content": "hi"},
                    "finish_reason": "stop"
                }]
            })))
            .mount(&server)
            .await;
        server
    });

    let racers: Vec<_> = (0..RACERS)
        .map(|_| {
            let base_url = server.uri();
            std::thread::spawn(move || {
                for _ in 0..TURNS {
                    turn(&base_url);
                }
            })
        })
        .collect();

    for racer in racers {
        racer
            .join()
            .expect("no turn is handed a dead pooled connection");
    }
}

#[tokio::test]
async fn default_streaming_route_uses_the_configured_model_and_yields_chunks() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(header("authorization", "Bearer test-secret"))
        .and(body_partial_json(json!({
            "model": "configured-model",
            "stream": true,
            "stream_options": {"include_usage": true}
        })))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(
                    "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hello\"},\"finish_reason\":null}]}\n\ndata: [DONE]\n\n",
                ),
        )
        .expect(1)
        .mount(&server)
        .await;

    let client = LlmClient::new(LlmConfig::new(
        server.uri(),
        "configured-model",
        "test-secret",
    ));
    let chunks = client
        .chat_stream(&[Message::user("hi")], None)
        .await
        .expect("the streaming request is accepted")
        .collect::<Vec<_>>()
        .await;

    assert_eq!(chunks.len(), 1);
    assert_eq!(
        chunks[0].as_ref().unwrap().choices[0]
            .delta
            .content
            .as_deref(),
        Some("hello")
    );
}

#[tokio::test]
async fn model_specific_streaming_route_preserves_provider_errors() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(body_partial_json(json!({
            "model": "requested-model",
            "stream": true
        })))
        .respond_with(ResponseTemplate::new(429).set_body_string("capacity exhausted"))
        .expect(1)
        .mount(&server)
        .await;

    let client = LlmClient::new(LlmConfig::new(server.uri(), "gpt-4", ""));
    let error = match client
        .chat_stream_with_model("requested-model", &[Message::user("hi")], None)
        .await
    {
        Ok(_) => panic!("the provider rejection must reach the caller"),
        Err(error) => error,
    };

    assert!(matches!(
        error,
        LlmError::Api {
            status: 429,
            ref message
        } if message == "capacity exhausted"
    ));
}
