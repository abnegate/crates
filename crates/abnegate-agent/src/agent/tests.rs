use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use abnegate_llm::LlmClient;
use abnegate_llm::LlmConfig;
use async_trait::async_trait;
use serde_json::Value;
use serde_json::json;
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
        client: LlmClient::new(LlmConfig::new(format!("{}/v1", server.uri()), "test", "")),
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

const RECORDING: &str = "recording";
const SLOW: &str = "slow";
const ASKING: &str = "asking";

/// A tool that notes each call it runs, at the tier it is given.
struct Recording {
    tier: crate::tools::Tier,
    runs: Arc<AtomicUsize>,
}

#[async_trait]
impl Tool for Recording {
    fn name(&self) -> &str {
        RECORDING
    }

    fn description(&self) -> &str {
        "Records that it ran."
    }

    fn parameters_schema(&self) -> Value {
        json!({"type": "object", "properties": {}})
    }

    fn tier(&self) -> crate::tools::Tier {
        self.tier
    }

    async fn execute(
        &self,
        _parameters: Value,
        _context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        self.runs.fetch_add(1, Ordering::SeqCst);
        Ok(ToolResult::success("ran"))
    }
}

fn recording(tier: crate::tools::Tier) -> (ToolRegistry, Arc<AtomicUsize>) {
    let runs = Arc::new(AtomicUsize::new(0));
    let mut tools = ToolRegistry::new();
    tools.register(Arc::new(Recording {
        tier,
        runs: Arc::clone(&runs),
    }));
    (tools, runs)
}

/// A callback that allows everything, as an application that has asked its
/// user would.
struct Approving;

impl super::AgentCallback for Approving {
    fn on_phase_change(&self, _phase: super::AgentPhase, _message: Option<&str>) {}
    fn on_tool_call(&self, _tool_name: &str, _arguments: &str) {}
    fn on_tool_result(&self, _tool_name: &str, _result: &ToolResult) {}
    fn on_response(&self, _response: &str) {}

    fn approve(&self, _call: &abnegate_llm::ToolCall, _tier: crate::tools::Tier) -> bool {
        true
    }
}

/// Nothing asked before a host-tier call ran: the tier said it should be
/// confirmed, and the loop ran it anyway. A caller that does not answer the
/// question now gets the safe answer.
#[tokio::test]
async fn a_call_that_needs_confirmation_is_refused_unless_the_callback_approves_it() {
    let script = || vec![calling(&[(RECORDING, json!({}))]), answer("done")];
    let unwatched = provider(script()).await;
    let (tools, runs) = recording(crate::tools::Tier::Host);

    let state = agent(&unwatched, tools)
        .run("Go.", &NoOpCallback)
        .await
        .expect("a refused call does not end the run");

    assert_eq!(
        runs.load(Ordering::SeqCst),
        0,
        "the host call ran unapproved"
    );
    let results = tool_results(&state);
    assert!(results[0].contains("not approved"), "{results:?}");

    let watched = provider(script()).await;
    let (tools, runs) = recording(crate::tools::Tier::Host);
    agent(&watched, tools).run("Go.", &Approving).await.unwrap();
    assert_eq!(runs.load(Ordering::SeqCst), 1, "an approved call runs");
}

#[tokio::test]
async fn a_call_that_needs_no_confirmation_runs_without_asking() {
    let provider = provider(vec![calling(&[(RECORDING, json!({}))]), answer("done")]).await;
    let (tools, runs) = recording(crate::tools::Tier::Read);

    agent(&provider, tools)
        .run("Go.", &NoOpCallback)
        .await
        .unwrap();

    assert_eq!(runs.load(Ordering::SeqCst), 1);
}

struct Slow;

#[async_trait]
impl Tool for Slow {
    fn name(&self) -> &str {
        SLOW
    }

    fn description(&self) -> &str {
        "Never finishes."
    }

    fn parameters_schema(&self) -> Value {
        json!({"type": "object", "properties": {}})
    }

    fn timeout(&self, _context: &ToolContext) -> std::time::Duration {
        std::time::Duration::from_millis(100)
    }

    async fn execute(
        &self,
        _parameters: Value,
        _context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        tokio::time::sleep(std::time::Duration::from_secs(3_600)).await;
        Ok(ToolResult::success("finished"))
    }
}

/// `Tool::timeout` was documented as the bound a caller applies, and the
/// loop never applied it: a wedged tool held the run open for good.
#[tokio::test]
async fn a_tool_past_its_timeout_fails_its_call() {
    let provider = provider(vec![calling(&[(SLOW, json!({}))]), answer("moved on")]).await;
    let mut tools = ToolRegistry::new();
    tools.register(Arc::new(Slow));

    let started = std::time::Instant::now();
    let state = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        agent(&provider, tools).run("Go.", &NoOpCallback),
    )
    .await
    .expect("the loop does not wait on the tool")
    .expect("a timed-out call does not end the run");

    assert!(started.elapsed() < std::time::Duration::from_secs(10));
    assert_eq!(state.final_response.as_deref(), Some("moved on"));
    let results = tool_results(&state);
    assert!(results[0].contains("timed out"), "{results:?}");
}

struct Asking;

#[async_trait]
impl Tool for Asking {
    fn name(&self) -> &str {
        ASKING
    }

    fn description(&self) -> &str {
        "Asks the user something."
    }

    fn parameters_schema(&self) -> Value {
        json!({"type": "object", "properties": {}})
    }

    fn ends_turn(&self) -> bool {
        true
    }

    async fn execute(
        &self,
        _parameters: Value,
        _context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        Ok(ToolResult::success("Which colour?"))
    }
}

/// `ends_turn` promised that nothing queued behind the call would run and no
/// further model round would follow, and the loop ignored it.
#[tokio::test]
async fn a_tool_that_ends_the_turn_stops_the_loop() {
    let provider = provider(vec![
        calling(&[(ASKING, json!({})), (RECORDING, json!({}))]),
        answer("should never be asked for"),
    ])
    .await;
    let (mut tools, runs) = recording(crate::tools::Tier::Read);
    tools.register(Arc::new(Asking));

    let state = agent(&provider, tools)
        .run("Go.", &NoOpCallback)
        .await
        .expect("the turn ends cleanly");

    assert_eq!(
        provider.received.lock().unwrap().len(),
        1,
        "another round followed"
    );
    assert_eq!(
        runs.load(Ordering::SeqCst),
        0,
        "a call queued behind it ran"
    );
    assert!(state.finished);
    assert_eq!(state.final_response.as_deref(), Some("Which colour?"));
    let results = tool_results(&state);
    assert_eq!(
        results.len(),
        2,
        "every call still has a result: {results:?}"
    );
    assert!(results[1].contains("Not run"), "{results:?}");
}

/// Only `stop` ended a turn. An answer that finished for any other reason -
/// `length`, Anthropic's `end_turn`, or none given at all - was appended and
/// sent straight back, round after round, until the iteration budget ran out.
#[tokio::test]
async fn an_answer_ends_the_turn_whatever_reason_it_finished_for() {
    for reason in [
        json!(null),
        json!("length"),
        json!("end_turn"),
        json!("stop"),
    ] {
        let provider = provider(vec![completion(
            json!({"role": "assistant", "content": "the answer"}),
            reason.clone(),
        )])
        .await;

        let state = agent(&provider, ToolRegistry::new())
            .run("Go.", &NoOpCallback)
            .await
            .unwrap_or_else(|error| panic!("{reason}: {error}"));

        assert_eq!(
            state.final_response.as_deref(),
            Some("the answer"),
            "{reason}"
        );
        assert_eq!(provider.received.lock().unwrap().len(), 1, "{reason}");
    }
}

/// An empty call list with no text asked for nothing, so the loop sent the
/// same request again, fifty times over.
#[tokio::test]
async fn a_model_that_keeps_answering_with_nothing_fails_the_turn_soon() {
    for reply in [
        completion(
            json!({"role": "assistant", "content": null, "tool_calls": []}),
            json!("tool_calls"),
        ),
        completion(
            json!({"role": "assistant", "content": "   "}),
            json!("stop"),
        ),
        completion(
            json!({"role": "assistant", "content": "calling it now"}),
            json!("tool_calls"),
        ),
        json!({"id": "reply", "object": "chat.completion", "created": 0, "model": "test", "choices": []}),
    ] {
        let provider = provider(vec![reply.clone()]).await;

        let error = agent(&provider, ToolRegistry::new())
            .run("Go.", &NoOpCallback)
            .await
            .expect_err("nothing usable never becomes an answer");

        assert!(
            matches!(error, super::AgentError::Empty),
            "{reply}: {error}"
        );
        assert_eq!(
            provider.received.lock().unwrap().len(),
            super::r#loop::MAX_EMPTY_RESPONSES,
            "{reply}"
        );
    }
}

#[tokio::test]
async fn a_round_with_something_in_it_resets_the_count_of_empty_ones() {
    let nothing = completion(
        json!({"role": "assistant", "content": null, "tool_calls": []}),
        json!("tool_calls"),
    );
    let (tools, _) = recording(crate::tools::Tier::Read);
    let provider = provider(vec![
        nothing.clone(),
        nothing.clone(),
        calling(&[(RECORDING, json!({}))]),
        nothing.clone(),
        nothing,
        answer("got there"),
    ])
    .await;

    let state = agent(&provider, tools)
        .run("Go.", &NoOpCallback)
        .await
        .expect("no three empty rounds in a row");

    assert_eq!(state.final_response.as_deref(), Some("got there"));
}

/// `AgentConfig::temperature` was never sent anywhere.
#[tokio::test]
async fn the_configured_temperature_is_the_one_requested() {
    let provider = provider(vec![answer("done")]).await;
    let config = AgentConfig {
        temperature: Some(0.25),
        ..AgentConfig::default()
    };
    Agent::new(
        provider.client.clone(),
        ToolRegistry::new(),
        config,
        ToolContext::default(),
    )
    .run("Go.", &NoOpCallback)
    .await
    .unwrap();

    let requests = provider.received.lock().unwrap();
    assert_eq!(requests[0]["temperature"], json!(0.25));
}
