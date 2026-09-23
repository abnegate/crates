use abnegate_llm::LlmClient;
use abnegate_llm::LlmConfig;
use async_trait::async_trait;
use serde_json::Value;
use serde_json::json;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::Request;
use wiremock::Respond;
use wiremock::ResponseTemplate;
use wiremock::matchers::method;

use super::Agent;
use super::AgentConfig;
use super::NoOpCallback;
use crate::tools::Tool;
use crate::tools::ToolContext;
use crate::tools::ToolError;
use crate::tools::ToolRegistry;
use crate::tools::ToolResult;

const PANICKING: &str = "panicking";

/// Replies to each request with the next scripted body, repeating the last
/// once the script runs out, and keeps every request it was sent.
struct Script {
    replies: Vec<Value>,
    next: AtomicUsize,
    received: Arc<Mutex<Vec<Value>>>,
}

impl Respond for Script {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        if let Ok(body) = serde_json::from_slice::<Value>(&request.body) {
            self.received.lock().expect("request log").push(body);
        }
        let index = self.next.fetch_add(1, Ordering::SeqCst);
        let reply = self
            .replies
            .get(index)
            .or(self.replies.last())
            .cloned()
            .unwrap_or_else(|| answer("done"));
        ResponseTemplate::new(200).set_body_json(reply)
    }
}

/// A provider that answers with `replies` in order.
pub(super) struct Provider {
    pub(super) client: LlmClient,
    pub(super) received: Arc<Mutex<Vec<Value>>>,
    _server: MockServer,
}

pub(super) async fn provider(replies: Vec<Value>) -> Provider {
    let server = MockServer::start().await;
    let received = Arc::new(Mutex::new(Vec::new()));
    Mock::given(method("POST"))
        .respond_with(Script {
            replies,
            next: AtomicUsize::new(0),
            received: Arc::clone(&received),
        })
        .mount(&server)
        .await;
    Provider {
        client: LlmClient::new(LlmConfig {
            base_url: format!("{}/v1", server.uri()),
            default_model: "test".to_string(),
            ..LlmConfig::default()
        }),
        received,
        _server: server,
    }
}

fn completion(message: Value, finish_reason: Value) -> Value {
    json!({
        "id": "reply",
        "object": "chat.completion",
        "created": 0,
        "model": "test",
        "choices": [{"index": 0, "message": message, "finish_reason": finish_reason}]
    })
}

pub(super) fn answer(content: &str) -> Value {
    completion(
        json!({"role": "assistant", "content": content}),
        json!("stop"),
    )
}

pub(super) fn calling(calls: &[(&str, Value)]) -> Value {
    let calls: Vec<Value> = calls
        .iter()
        .enumerate()
        .map(|(index, (name, arguments))| {
            json!({
                "id": format!("call{index}"),
                "type": "function",
                "function": {"name": name, "arguments": arguments.to_string()}
            })
        })
        .collect();
    completion(
        json!({"role": "assistant", "content": null, "tool_calls": calls}),
        json!("tool_calls"),
    )
}

/// The tool results the run fed back to the model, in order.
pub(super) fn tool_results(state: &super::AgentState) -> Vec<String> {
    state
        .messages
        .iter()
        .filter(|message| message.role == abnegate_llm::Role::Tool)
        .filter_map(|message| message.content.clone())
        .collect()
}

pub(super) fn agent(provider: &Provider, tools: ToolRegistry) -> Agent {
    Agent::new(
        provider.client.clone(),
        tools,
        AgentConfig::default(),
        ToolContext::default(),
    )
}

struct Panicking;

#[async_trait]
impl Tool for Panicking {
    fn name(&self) -> &str {
        PANICKING
    }

    fn description(&self) -> &str {
        "Panics."
    }

    fn parameters_schema(&self) -> Value {
        json!({"type": "object", "properties": {}})
    }

    async fn execute(
        &self,
        _parameters: Value,
        _context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        panic!("{PANICKING} always panics")
    }
}

/// A tool that panics fails its own call. It used to unwind through the loop
/// and take the whole run, and every result gathered so far, with it.
#[tokio::test]
async fn a_tool_that_panics_fails_its_call_and_the_run_carries_on() {
    let provider = provider(vec![
        calling(&[(PANICKING, json!({}))]),
        answer("recovered"),
    ])
    .await;
    let mut tools = ToolRegistry::new();
    tools.register(Arc::new(Panicking));

    let state = agent(&provider, tools)
        .run("Go.", &NoOpCallback)
        .await
        .expect("a panicking tool does not end the run");

    assert_eq!(state.final_response.as_deref(), Some("recovered"));
    assert_eq!(provider.received.lock().unwrap().len(), 2);
    let results = tool_results(&state);
    assert_eq!(results.len(), 1, "{results:?}");
    assert!(
        results[0].contains("failed unexpectedly") && results[0].contains(PANICKING),
        "{results:?}"
    );
}
