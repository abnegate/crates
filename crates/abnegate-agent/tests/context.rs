use std::sync::Arc;

use abnegate_agent::Agent;
use abnegate_agent::AgentConfig;
use abnegate_agent::NoOpCallback;
use abnegate_agent::ToolContext;
use abnegate_agent::ToolRegistry;
use abnegate_agent::context;
use abnegate_agent::context::ContextError;
use abnegate_agent::context::ContextSource;
use abnegate_agent::context::ContextStatus;
use abnegate_agent::context::Coverage;
use abnegate_agent::context::Entry;
use abnegate_agent::context::Policy;
use abnegate_agent::context::Summary;
use abnegate_llm::Effort;
use abnegate_llm::HttpProvider;
use abnegate_llm::LlmClient;
use abnegate_llm::LlmConfig;
use abnegate_llm::Message;
use abnegate_llm::RequestOptions;
use abnegate_llm::Role;
use abnegate_llm::ToolCall;
use abnegate_llm::ToolDefinition;
use abnegate_llm::provider::testing::StubProvider;
use futures::StreamExt;
use serde_json::Value;
use serde_json::json;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tokio::sync::Mutex;
use tokio::sync::Notify;

struct Provider {
    client: LlmClient,
    requests: Arc<Mutex<Vec<Value>>>,
    task: tokio::task::JoinHandle<()>,
}

impl Provider {
    /// A completion provider over the recorded endpoint.
    fn completions(&self) -> HttpProvider {
        HttpProvider::new("test", self.client.clone())
    }
}

impl Drop for Provider {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// The first line of the request compaction sends for a summary.
const SUMMARY_INSTRUCTIONS: &str = "Maintain a compact historical conversation record.";

fn asks_for_a_summary(request: &Value) -> bool {
    request["messages"][0]["content"]
        .as_str()
        .is_some_and(|content| content.starts_with(SUMMARY_INSTRUCTIONS))
}

async fn provider(
    answer: impl Fn(&Value) -> String + Send + Sync + 'static,
    stream: bool,
) -> Provider {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let recorded = requests.clone();
    let task = tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            let (start, length) = loop {
                let mut buffer = [0; 4096];
                let read = socket.read(&mut buffer).await.unwrap();
                if read == 0 {
                    return;
                }
                bytes.extend_from_slice(&buffer[..read]);
                if let Some(end) = bytes.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                    let header = std::str::from_utf8(&bytes[..end]).unwrap();
                    let length = header
                        .lines()
                        .find_map(|line| {
                            line.to_lowercase()
                                .strip_prefix("content-length:")
                                .map(|length| length.trim().parse::<usize>().unwrap())
                        })
                        .unwrap();
                    break (end + 4, length);
                }
            };
            while bytes.len() < start + length {
                let mut buffer = [0; 4096];
                let read = socket.read(&mut buffer).await.unwrap();
                if read == 0 {
                    return;
                }
                bytes.extend_from_slice(&buffer[..read]);
            }
            let request = serde_json::from_slice::<Value>(&bytes[start..start + length]).unwrap();
            recorded.lock().await.push(request.clone());
            let body = answer(&request);
            let mime = if stream {
                "text/event-stream"
            } else {
                "application/json"
            };
            socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
        }
    });
    Provider {
        client: LlmClient::new(
            LlmConfig::new(format!("http://{address}/v1"), "test", "").with_maximum_tokens(1024),
        ),
        requests,
        task,
    }
}

fn structured() -> String {
    json!({"objective":"Complete requested work", "constraints":["Keep all corrections"], "corrections":["Use corrected path"], "decisions":[], "completed":["Read evidence"], "evidence":["Error: failed command; original remains retrievable"], "failed":["First attempt failed"], "pending":["Check the result"], "questions":[]}).to_string()
}

fn response(content: String) -> String {
    json!({"id":"summary", "object":"chat.completion", "created":0, "model":"test", "choices":[{"index":0,"message":{"role":"assistant","content":content},"finish_reason":"stop"}]}).to_string()
}

fn entry(id: &str, message: Message, preserve: bool, consumed: bool) -> Entry {
    Entry::new(id, message)
        .with_preserve(preserve)
        .with_consumed(consumed)
}

fn call(id: &str) -> ToolCall {
    ToolCall::function(id, "read_file", r#"{"path":"evidence.txt"}"#)
}

fn policy(limit: u64) -> Policy {
    Policy::new(Some(limit), 1024, ContextSource::Configured)
}

fn active_history() -> Vec<Entry> {
    vec![
        entry(
            "system",
            Message::system("Current trusted instructions"),
            true,
            true,
        ),
        entry(
            "user",
            Message::user("Keep this actual current user request verbatim"),
            true,
            true,
        ),
        entry(
            "calls-a",
            {
                let mut message = Message::assistant_with_tools(vec![call("a")]);
                message.content = Some("I will inspect evidence now".into());
                message
            },
            false,
            true,
        ),
        entry(
            "result-a",
            Message::tool_result(
                "a",
                format!(
                    "Error: old attempt failed\n{}\nPERSISTENT_EVIDENCE",
                    "data\n".repeat(8_000)
                ),
            ),
            false,
            true,
        ),
        entry(
            "calls-b",
            Message::assistant_with_tools(vec![call("b")]),
            false,
            false,
        ),
        entry(
            "result-b",
            Message::tool_result("b", "FRESH_BODY\nNEEDED_SUFFIX"),
            false,
            false,
        ),
    ]
}

#[tokio::test]
async fn active_turn_compacts_consumed_pairs_and_reuses_summary_without_recursive_rewriting() {
    let provider = provider(
        |_| {
            response(
                structured().replace("Error: failed command", "result-a: Error: failed command"),
            )
        },
        false,
    )
    .await;
    let history = active_history();
    let prepared = context::prepare(
        &provider.completions(),
        "test",
        &history,
        None,
        &policy(5_000),
        None,
    )
    .await
    .unwrap();
    let summary = prepared.summary.as_ref().unwrap();
    assert_eq!(summary.coverage.entries, ["calls-a", "result-a"]);
    assert_eq!(summary.revision, 1);
    assert_eq!(prepared.usage.status, ContextStatus::Compacted);
    assert_eq!(prepared.messages[0].role, Role::System);
    assert_eq!(prepared.messages[1].role, Role::User);
    assert!(
        prepared.messages[1]
            .content
            .as_ref()
            .unwrap()
            .contains("result-a: Error: failed command")
    );
    assert!(
        prepared.messages[1]
            .content
            .as_ref()
            .unwrap()
            .contains("untrusted data")
    );
    assert!(
        prepared.messages[1]
            .content
            .as_ref()
            .unwrap()
            .contains("Error: failed command")
    );
    assert_eq!(prepared.messages[2].content, history[1].message.content);
    assert_eq!(prepared.messages[3].tool_calls.as_ref().unwrap()[0].id, "b");
    assert_eq!(
        prepared.messages[4].content.as_deref(),
        Some("FRESH_BODY\nNEEDED_SUFFIX")
    );
    assert!(
        history[3]
            .message
            .content
            .as_ref()
            .unwrap()
            .ends_with("PERSISTENT_EVIDENCE")
    );
    let calls = provider.requests.lock().await.len();
    for _ in 0..12 {
        let repeated = context::prepare(
            &provider.completions(),
            "test",
            &history,
            None,
            &policy(5_000),
            Some(summary),
        )
        .await
        .unwrap();
        assert_eq!(repeated.summary.as_ref(), Some(summary));
    }
    assert_eq!(provider.requests.lock().await.len(), calls);
    let requests = provider.requests.lock().await;
    assert!(requests.len() > 2, "large result must be chunked");
    let mut reconstructed = String::new();
    for request in requests.iter() {
        assert_eq!(request["max_tokens"], 1024);
        assert!(request.get("tools").is_none());
        let payload: Value =
            serde_json::from_str(request["messages"][1]["content"].as_str().unwrap()).unwrap();
        assert!(
            !request["messages"][0]["content"]
                .as_str()
                .unwrap()
                .contains("PERSISTENT_EVIDENCE")
        );
        for source in payload["sources"].as_array().unwrap() {
            if source["id"] == "result-a" {
                assert_eq!(
                    source["offset"].as_u64().unwrap(),
                    reconstructed.len() as u64
                );
                reconstructed.push_str(source["fragment"].as_str().unwrap());
            }
        }
        let messages: Vec<Entry> = request["messages"]
            .as_array()
            .unwrap()
            .iter()
            .enumerate()
            .map(|(index, message)| {
                entry(
                    &index.to_string(),
                    serde_json::from_value(message.clone()).unwrap(),
                    true,
                    false,
                )
            })
            .collect();
        assert!(
            context::estimate("test", &messages, None, &policy(5_000), None).used
                <= policy(5_000).threshold().unwrap()
        );
    }
    assert!(reconstructed.contains("PERSISTENT_EVIDENCE"));
}

#[tokio::test]
async fn a_later_user_allows_previous_protected_user_and_consumed_groups_into_delta() {
    let provider = provider(|_| response(structured()), false).await;
    let mut history = active_history();
    let first = context::prepare(
        &provider.completions(),
        "test",
        &history,
        None,
        &policy(5_000),
        None,
    )
    .await
    .unwrap();
    let previous = first.summary.unwrap();
    history[1].preserve = false;
    history[4].consumed = true;
    history[5].consumed = true;
    history[5].message.content = Some("more evidence\n".repeat(1_000));
    history.push(entry(
        "next-user",
        Message::user("New actual request"),
        true,
        false,
    ));
    let count = provider.requests.lock().await.len();
    let next = context::prepare(
        &provider.completions(),
        "test",
        &history,
        None,
        &policy(5_000),
        Some(&previous),
    )
    .await
    .unwrap();
    let summary = next.summary.unwrap();
    assert_eq!(summary.revision, 2);
    assert_eq!(
        summary.coverage.entries,
        ["user", "calls-a", "result-a", "calls-b", "result-b"]
    );
    for request in &provider.requests.lock().await[count..] {
        let input: Value =
            serde_json::from_str(request["messages"][1]["content"].as_str().unwrap()).unwrap();
        for source in input["sources"].as_array().unwrap() {
            assert_ne!(source["id"], "result-a");
            assert_ne!(source["id"], "calls-a");
        }
        assert!(!input["previous_state"].as_str().unwrap().is_empty());
    }
}

#[tokio::test]
async fn fresh_seven_results_and_long_suffix_reach_actual_provider_unchanged() {
    let provider = provider(|_| response("Done".into()), false).await;
    let body = format!("{}\nTAIL_EVIDENCE", "😀".repeat(8_200));
    let mut history = vec![
        entry("user", Message::user("Read all seven"), true, true),
        entry(
            "calls",
            Message::assistant_with_tools((0..7).map(|index| call(&index.to_string())).collect()),
            false,
            false,
        ),
    ];
    history.extend((0..7).map(|index| {
        entry(
            &format!("result-{index}"),
            Message::tool_result(index.to_string(), body.clone()),
            false,
            false,
        )
    }));
    let settings = policy(300_000);
    let prepared = context::prepare(
        &provider.completions(),
        "test",
        &history,
        None,
        &settings,
        None,
    )
    .await
    .unwrap();
    provider
        .client
        .chat_with_options(
            "test",
            &prepared.messages,
            None,
            RequestOptions::new(settings.reserved),
        )
        .await
        .unwrap();
    let requests = provider.requests.lock().await;
    assert_eq!(requests.len(), 1);
    for message in &requests[0]["messages"].as_array().unwrap()[2..] {
        assert_eq!(message["content"], body);
    }
}

#[tokio::test]
async fn unconsumed_overflow_skips_summarizer_even_when_older_history_is_eligible() {
    let provider = provider(|_| panic!("provider must not be called"), false).await;
    let history = vec![
        entry("old-user", Message::user("prior question"), false, true),
        entry(
            "old-assistant",
            Message::assistant("prior answer"),
            false,
            true,
        ),
        entry(
            "call",
            Message::assistant_with_tools(vec![call("a")]),
            false,
            false,
        ),
        entry(
            "result",
            Message::tool_result("a", "r".repeat(20_000)),
            false,
            false,
        ),
        entry("current", Message::user("Continue"), true, false),
    ];
    match context::prepare(
        &provider.completions(),
        "test",
        &history,
        None,
        &policy(5_000),
        None,
    )
    .await
    {
        Err(ContextError::Capacity { reason, .. }) => {
            assert!(reason.contains("cannot be compacted"));
            assert!(!reason.contains("Summary could not free enough"));
        }
        other => panic!("expected uncompactable remainder, got {other:?}"),
    }
}

#[tokio::test]
async fn indivisible_fresh_result_and_latest_user_overflow_do_not_call_summarizer() {
    let provider = provider(|_| panic!("provider must not be called"), false).await;
    for history in [
        vec![entry(
            "user",
            Message::user("u".repeat(20_000)),
            true,
            false,
        )],
        vec![
            entry(
                "call",
                Message::assistant_with_tools(vec![call("a")]),
                false,
                false,
            ),
            entry(
                "result",
                Message::tool_result("a", "r".repeat(20_000)),
                false,
                false,
            ),
        ],
    ] {
        assert!(matches!(
            context::prepare(
                &provider.completions(),
                "test",
                &history,
                None,
                &policy(5_000),
                None
            )
            .await,
            Err(ContextError::Capacity { .. })
        ));
    }
}

#[tokio::test]
async fn invalid_summary_never_advances_coverage_or_mutates_history() {
    for answer in [
        "".to_owned(),
        "{}".into(),
        "not JSON".into(),
        structured().replace("Complete requested work", &"x".repeat(5_000)),
    ] {
        let provider = provider(move |_| response(answer.clone()), false).await;
        let history = active_history();
        let fingerprint =
            context::coverage(&history, &["calls-a".into(), "result-a".into()]).unwrap();
        assert!(
            context::prepare(
                &provider.completions(),
                "test",
                &history,
                None,
                &policy(5_000),
                None
            )
            .await
            .is_err()
        );
        assert_eq!(
            context::coverage(&history, &fingerprint.entries).unwrap(),
            fingerprint
        );
    }
}

#[tokio::test]
async fn failed_summary_replays_original_history_only_when_it_still_fits() {
    let provider = provider(|_| response("".into()), false).await;
    let history = vec![
        entry("old", Message::user("x".repeat(12_800)), false, true),
        entry("current", Message::user("Current"), true, false),
    ];
    let prepared = context::prepare(
        &provider.completions(),
        "test",
        &history,
        None,
        &policy(5_000),
        None,
    )
    .await
    .unwrap();
    assert!(prepared.summary.is_none());
    assert_eq!(prepared.usage.status, ContextStatus::Blocked);
    assert_eq!(prepared.messages[0].content, history[0].message.content);
    assert!(
        prepared
            .usage
            .reason
            .unwrap()
            .contains("original history remains intact")
    );
}

#[test]
fn coverage_rejects_changed_fields_missing_ids_reorder_partial_pairs_and_protected_data() {
    let history = active_history();
    let ids = ["calls-a".into(), "result-a".into()];
    let summary = Summary::new(structured(), context::coverage(&history, &ids).unwrap(), 1);
    context::validate(&history, Some(&summary)).unwrap();
    for variant in 0..5 {
        let mut changed = history.clone();
        match variant {
            0 => changed[3].message.content = Some("altered evidence".into()),
            1 => {
                changed[2].message.tool_calls.as_mut().unwrap()[0]
                    .function
                    .arguments = "{\"path\":\"other\"}".into()
            }
            2 => changed[3].message.images.push("image-reference".into()),
            3 => changed[3].message.name = Some("changed-name".into()),
            _ => changed[2].message.content = Some("changed assistant prose".into()),
        }
        assert!(context::validate(&changed, Some(&summary)).is_err());
    }
    for ids in [
        vec!["result-a".into()],
        vec!["calls-b".into(), "result-b".into()],
        vec!["user".into()],
        vec!["system".into()],
    ] {
        let mut invalid = summary.clone();
        invalid.coverage = context::coverage(&history, &ids).unwrap();
        assert!(context::validate(&history, Some(&invalid)).is_err());
    }
    assert!(context::coverage(&history, &["result-a".into(), "calls-a".into()]).is_err());
    assert!(context::coverage(&history, &["missing".into()]).is_err());
    assert!(context::coverage(&history, &["calls-a".into(), "calls-a".into()]).is_err());
}

#[test]
fn append_only_history_preserves_fingerprint_and_image_accounting_is_unknown() {
    let mut history = active_history();
    let summary = Summary::new(
        structured(),
        context::coverage(&history, &["calls-a".into(), "result-a".into()]).unwrap(),
        1,
    );
    let mut image = Message::user("View attachment");
    image.images.push("data:image/png;base64,example".into());
    history.push(entry("image", image, true, false));
    context::validate(&history, Some(&summary)).unwrap();
    let estimate = context::estimate("test", &history, None, &policy(5_000), Some(&summary));
    assert_eq!(estimate.breakdown.attachments, None);
    assert!(estimate.incomplete);
    assert!(estimate.estimated);
    assert_eq!(estimate.used, estimate.breakdown.total());
}

#[tokio::test]
async fn unknown_capacity_does_not_guess_model_limits_or_discard_history() {
    let provider = provider(|_| panic!("unknown budget must not summarize"), false).await;
    let history = active_history();
    let settings = Policy::new(None, 4096, ContextSource::Unknown);
    let prepared = context::prepare(
        &provider.completions(),
        "gpt-future-1m",
        &history,
        None,
        &settings,
        None,
    )
    .await
    .unwrap();
    assert_eq!(prepared.usage.limit, None);
    assert_eq!(prepared.usage.threshold, None);
    assert_eq!(prepared.usage.remaining, None);
    assert_eq!(prepared.usage.status, ContextStatus::Unavailable);
    assert_eq!(prepared.messages[3].content, history[3].message.content);
}

#[test]
fn tools_and_message_framing_match_projection_estimates_and_threshold_is_saturating() {
    let history = active_history();
    let summary = Summary::new(
        structured(),
        context::coverage(&history, &["calls-a".into(), "result-a".into()]).unwrap(),
        1,
    );
    let tools = [ToolDefinition::function(
        "read",
        "Read \"quoted\" paths\n",
        json!({"type":"object","properties":{"path":{"type":"string"}}}),
    )];
    let usage = context::estimate(
        "test",
        &history,
        Some(&tools),
        &policy(5_000),
        Some(&summary),
    );
    let projected: Vec<_> = context::project(&history, Some(&summary))
        .unwrap()
        .into_iter()
        .enumerate()
        .map(|(index, message)| entry(&index.to_string(), message, true, false))
        .collect();
    let projected_usage = context::estimate("test", &projected, Some(&tools), &policy(5_000), None);
    assert_eq!(usage.used, projected_usage.used);
    assert_eq!(
        usage.breakdown.tools,
        (serde_json::to_string(&tools).unwrap().len() as u64).div_ceil(4)
    );
    assert_eq!(usage.used, usage.breakdown.total());
    assert_eq!(
        policy(u64::MAX).threshold(),
        Some((u64::MAX - 1024) - (u64::MAX - 1024) / 5)
    );
    assert_eq!(policy(100).input_limit(), Some(0));
    assert_eq!(policy(100).threshold(), Some(0));
    assert_eq!(
        context::estimate("test", &history, None, &policy(5_000), None).status,
        ContextStatus::Ready
    );
}

/// Compaction used to clear the agent's own client of its stop sequences and
/// reasoning before asking it for a summary. A provider cannot be cleared, so
/// the agent hands summaries to a summarizer set apart, and every round still
/// goes out with the stops and reasoning its provider sends.
#[tokio::test]
async fn stops_and_reasoning_reach_every_round_and_no_summary_from_a_summarizer_set_apart() {
    let provider = provider(
        |request| {
            if asks_for_a_summary(request) {
                response(structured())
            } else {
                response("Done".into())
            }
        },
        false,
    )
    .await;
    let stops = vec!["}".to_string(), "<|end|>".to_string()];
    let client = provider
        .client
        .clone()
        .with_stop(stops.clone())
        .with_temperature(0.7)
        .with_reasoning("test", Effort::Medium)
        .with_ollama_context("test", 5_000);
    let summarizer = client.clone().with_stop(Vec::new()).without_reasoning();
    let agent = Agent::new(
        Arc::new(HttpProvider::new("rounds", client)),
        "test",
        ToolRegistry::new(),
        AgentConfig::default().with_system_prompt("Be brief."),
        ToolContext::default(),
    )
    .with_context_policy(policy(5_000))
    .with_summarizer(Arc::new(HttpProvider::new("summaries", summarizer)));

    let mut state = agent
        .run("x".repeat(12_800), &NoOpCallback)
        .await
        .expect("the first turn still fits");
    agent
        .continue_run(&mut state, "Again.", &NoOpCallback)
        .await
        .expect("character stops must not truncate structured summaries");

    let summary = state.summary.as_ref().expect("the second turn compacted");
    assert_eq!(summary.revision, 1);
    let requests = provider.requests.lock().await;
    let (summaries, rounds): (Vec<&Value>, Vec<&Value>) = requests
        .iter()
        .partition(|request| asks_for_a_summary(request));
    assert!(!summaries.is_empty());
    assert_eq!(rounds.len(), 2, "one request a round");
    for summary in summaries {
        assert!(summary.get("stop").is_none(), "{summary}");
        assert!(summary.get("reasoning_effort").is_none(), "{summary}");
        assert_eq!(summary["temperature"], 0.0);
        assert_eq!(summary["num_ctx"], 5_000);
        assert_eq!(summary["max_tokens"], 1024);
    }
    for round in rounds {
        assert_eq!(round["stop"], json!(stops));
        assert_eq!(round["reasoning_effort"], "medium");
        assert_eq!(round["temperature"].as_f64().unwrap() as f32, 0.7);
        assert_eq!(round["num_ctx"], 5_000);
    }
}

/// Whatever provider writes a summary is asked for it of the model it was
/// given, with no tools offered and nothing sampled.
#[tokio::test]
async fn a_summary_is_asked_of_the_given_model_at_temperature_zero_without_tools() {
    let summarizer = StubProvider::answering("stub", structured());

    let prepared = context::prepare(
        &summarizer,
        "qwen3",
        &active_history(),
        None,
        &policy(5_000),
        None,
    )
    .await
    .unwrap();

    assert_eq!(prepared.usage.status, ContextStatus::Compacted);
    assert!(summarizer.calls() > 1, "the long result is chunked");
    let seen = summarizer.seen().unwrap();
    assert_eq!(seen.model, "qwen3");
    assert_eq!(seen.temperature, Some(0.0));
    assert!(seen.tools.is_none());
    assert_eq!(seen.options.reserved, 1024);
    assert!(
        seen.messages[0]
            .content
            .as_deref()
            .is_some_and(|content| content.starts_with(SUMMARY_INSTRUCTIONS))
    );
}

/// A summarizer that cannot answer leaves the history as it was while it
/// still fits, and its failure is reported once it does not.
#[tokio::test]
async fn a_failing_summarizer_blocks_compaction_and_reports_why_once_nothing_fits() {
    let summarizer = StubProvider::failing("stub", "the summarizer is offline");

    let prepared = context::prepare(
        &summarizer,
        "test",
        &[
            entry("old", Message::user("x".repeat(12_800)), false, true),
            entry("current", Message::user("Current"), true, false),
        ],
        None,
        &policy(5_000),
        None,
    )
    .await
    .unwrap();
    assert_eq!(prepared.usage.status, ContextStatus::Blocked);
    assert!(prepared.summary.is_none());

    let error = context::prepare(
        &summarizer,
        "test",
        &[
            entry("old", Message::user("x".repeat(40_000)), false, true),
            entry("current", Message::user("Current"), true, false),
        ],
        None,
        &policy(5_000),
        None,
    )
    .await
    .expect_err("history past the input limit cannot go out as it was");
    assert!(
        matches!(&error, ContextError::Provider(failure) if failure.to_string().contains("offline")),
        "{error:?}"
    );
}

#[tokio::test]
async fn runtime_context_is_model_bound_and_identical_on_summary_and_ordinary_requests() {
    let provider = provider(|_| response(structured()), false).await;
    let client = provider.client.clone().with_ollama_context("test", 5_000);
    let history = active_history();
    let prepared = context::prepare(
        &HttpProvider::new("test", client.clone()),
        "test",
        &history,
        None,
        &policy(5_000),
        None,
    )
    .await
    .unwrap();
    client
        .chat_with_options("test", &prepared.messages, None, RequestOptions::new(1024))
        .await
        .unwrap();
    client
        .chat_with_options(
            "other-provider",
            &[Message::user("Hi")],
            None,
            RequestOptions::new(512),
        )
        .await
        .unwrap();
    let requests = provider.requests.lock().await;
    for request in &requests[..requests.len() - 1] {
        assert_eq!(request["num_ctx"], 5_000);
        assert_eq!(request["max_tokens"], 1024);
    }
    assert!(requests.last().unwrap().get("num_ctx").is_none());
    assert_eq!(requests.last().unwrap()["max_tokens"], 512);
}

#[tokio::test]
async fn streaming_usage_after_finish_reason_is_preserved() {
    let provider = provider(|_| "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Hi\"},\"finish_reason\":null}]}\n\ndata: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: {\"choices\":[],\"usage\":{\"prompt_tokens\":42,\"completion_tokens\":2,\"total_tokens\":44}}\n\ndata: [DONE]\n\n".into(), true).await;
    let stream = provider
        .client
        .chat_stream_with_options(
            "test",
            &[Message::user("Hi")],
            None,
            RequestOptions::new(100),
        )
        .await
        .unwrap();
    let chunks: Vec<_> = stream.collect().await;
    assert_eq!(chunks.len(), 3);
    assert_eq!(
        chunks[2]
            .as_ref()
            .unwrap()
            .usage
            .as_ref()
            .unwrap()
            .prompt_tokens,
        42
    );
    assert_eq!(
        provider.requests.lock().await[0]["stream_options"]["include_usage"],
        true
    );
}

#[tokio::test]
async fn cancelling_summarization_keeps_checkpoint_and_canonical_evidence_unchanged() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let reached = Arc::new(Notify::new());
    let notified = reached.clone();
    let server = tokio::spawn(async move {
        let (_socket, _) = listener.accept().await.unwrap();
        notified.notify_one();
        std::future::pending::<()>().await;
    });
    let summarizer = HttpProvider::new(
        "test",
        LlmClient::new(LlmConfig::new(format!("http://{address}/v1"), "test", "")),
    );
    let history = active_history();
    let original = context::coverage(&history, &["calls-a".into(), "result-a".into()]).unwrap();
    let settings = policy(5_000);
    let mut future = Box::pin(context::prepare(
        &summarizer,
        "test",
        &history,
        None,
        &settings,
        None,
    ));
    tokio::select! { _ = reached.notified() => {}, _ = &mut future => panic!("summarizer should be pending") }
    drop(future);
    server.abort();
    assert_eq!(
        context::coverage(&history, &original.entries).unwrap(),
        original
    );
}

#[test]
fn canonical_agent_state_roundtrips_image_references_and_summary_separately() {
    let mut state = abnegate_agent::AgentState::new("View", None);
    state.messages[0]
        .images
        .push("data:image/png;base64,reference".into());
    state.summary = Some(Summary::new(
        structured(),
        Coverage::new(vec!["historical".into()], "example"),
        2,
    ));
    let serialized = serde_json::to_string(&state).unwrap();
    let restored: abnegate_agent::AgentState = serde_json::from_str(&serialized).unwrap();
    assert_eq!(restored.messages[0].images, state.messages[0].images);
    assert_eq!(restored.messages[0].content, state.messages[0].content);
    assert_eq!(restored.summary, state.summary);
}

#[tokio::test]
async fn many_small_messages_are_batched_into_one_summary_request() {
    let provider = provider(|_| response(structured()), false).await;
    let mut history: Vec<_> = (0..80)
        .map(|index| {
            entry(
                &format!("old-{index}"),
                Message::user("A short historical message"),
                false,
                true,
            )
        })
        .collect();
    history.push(entry(
        "current",
        Message::user("Current request"),
        true,
        false,
    ));
    let tools = [ToolDefinition::function(
        "read",
        "tool documentation ".repeat(1_500),
        json!({"type":"object"}),
    )];
    let settings = policy(11_000);
    assert!(
        context::estimate("test", &history, Some(&tools), &settings, None).used
            >= settings.threshold().unwrap()
    );
    let prepared = context::prepare(
        &provider.completions(),
        "test",
        &history,
        Some(&tools),
        &settings,
        None,
    )
    .await
    .unwrap();
    assert_eq!(prepared.summary.unwrap().coverage.entries.len(), 80);
    let requests = provider.requests.lock().await;
    assert_eq!(requests.len(), 1);
    let payload: Value =
        serde_json::from_str(requests[0]["messages"][1]["content"].as_str().unwrap()).unwrap();
    assert_eq!(payload["sources"].as_array().unwrap().len(), 80);
}

#[tokio::test]
async fn cli_continuation_preserves_assistant_prose_and_disambiguates_reused_provider_call_ids() {
    let provider = provider(|request| {
        if request["messages"].as_array().unwrap().last().unwrap()["role"] == "tool" { response("Finished".into()) }
        else { json!({"id":"reply", "object":"chat.completion", "created":0, "model":"test", "choices":[{"index":0,"message":{"role":"assistant","content":"I am checking the evidence","tool_calls":[{"id":"call0","type":"function","function":{"name":"read_file","arguments":"{}"}}]},"finish_reason":"tool_calls"}]}).to_string() }
    }, false).await;
    let agent = Agent::new(
        Arc::new(provider.completions()),
        "test",
        ToolRegistry::new(),
        AgentConfig::default(),
        ToolContext::default(),
    );
    let mut state = agent.run("First turn", &NoOpCallback).await.unwrap();
    agent
        .continue_run(&mut state, "Second turn", &NoOpCallback)
        .await
        .unwrap();
    let envelopes: Vec<_> = state
        .messages
        .iter()
        .filter(|message| message.tool_calls.is_some())
        .collect();
    assert_eq!(envelopes.len(), 2);
    assert_eq!(
        envelopes[0].content.as_deref(),
        Some("I am checking the evidence")
    );
    assert_eq!(envelopes[1].content, envelopes[0].content);
    assert_eq!(envelopes[0].tool_calls.as_ref().unwrap()[0].id, "call0");
    assert_ne!(envelopes[1].tool_calls.as_ref().unwrap()[0].id, "call0");
    let history: Vec<_> = state
        .messages
        .iter()
        .enumerate()
        .map(|(index, message)| entry(&index.to_string(), message.clone(), false, true))
        .collect();
    context::validate(&history, None).unwrap();
    let requests = provider.requests.lock().await;
    let followup = &requests[2]["messages"];
    assert!(
        followup
            .as_array()
            .unwrap()
            .iter()
            .any(|message| message["role"] == "tool" && message["tool_call_id"] == "call0")
    );
    assert_eq!(state.iteration, 2);
}

#[tokio::test]
async fn incomplete_or_duplicate_tool_outcomes_fail_before_inference() {
    let provider = provider(|_| panic!("invalid replay must not reach provider"), false).await;
    let missing = vec![
        entry(
            "calls",
            Message::assistant_with_tools(vec![call("a"), call("b")]),
            false,
            true,
        ),
        entry(
            "result-a",
            Message::tool_result("a", "only one completed"),
            false,
            true,
        ),
    ];
    let duplicate = vec![
        entry(
            "calls",
            Message::assistant_with_tools(vec![call("a")]),
            false,
            true,
        ),
        entry("result-a", Message::tool_result("a", "first"), false, true),
        entry(
            "result-again",
            Message::tool_result("a", "duplicate"),
            false,
            true,
        ),
    ];
    for history in [missing, duplicate] {
        assert!(matches!(
            context::prepare(
                &provider.completions(),
                "test",
                &history,
                None,
                &policy(5_000),
                None
            )
            .await,
            Err(ContextError::Integrity(_))
        ));
    }
}

#[tokio::test]
async fn fenced_structured_summary_is_validated_with_deterministic_sampling() {
    let provider = provider(
        |_| response(format!("```json\n{}\n```", structured())),
        false,
    )
    .await;
    let prepared = context::prepare(
        &provider.completions(),
        "test",
        &active_history(),
        None,
        &policy(5_000),
        None,
    )
    .await
    .unwrap();
    assert!(prepared.summary.unwrap().content.starts_with('{'));
    for request in provider.requests.lock().await.iter() {
        assert_eq!(request["temperature"], 0.0);
    }
    assert_eq!(provider.client.config().temperature, 0.7);
}

#[tokio::test]
async fn eighty_text_rows_compact_at_4096_without_spending_context_on_unretrievable_ids() {
    let provider = provider(|_| response(structured()), false).await;
    let mut history = vec![entry(
        "system",
        Message::system("Production chat instruction. ".repeat(45)),
        true,
        true,
    )];
    for index in 0..80 {
        history.push(entry(
            &format!("legacy:{}", uuid::Uuid::new_v4()),
            Message::user(format!(
                "Historical row {index}: {}",
                "important detail ".repeat(4)
            )),
            false,
            true,
        ));
    }
    history.push(entry(
        "current",
        Message::user("Continue the actual request"),
        true,
        false,
    ));
    let prepared = context::prepare(
        &provider.completions(),
        "test",
        &history,
        None,
        &policy(4096),
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        prepared.summary.as_ref().unwrap().coverage.entries.len(),
        80
    );
    assert!(prepared.usage.used < policy(4096).threshold().unwrap());
    assert!(
        !prepared.messages[1]
            .content
            .as_ref()
            .unwrap()
            .contains("legacy:")
    );
    context::validate(&history, prepared.summary.as_ref()).unwrap();
}

#[tokio::test]
async fn sustained_tool_history_keeps_full_coverage_without_an_unbounded_prompt_catalog() {
    let provider = provider(|_| response(structured()), false).await;
    let mut history = vec![entry(
        "system",
        Message::system("Continue accurately"),
        true,
        true,
    )];
    let mut covered = Vec::new();
    let mut relevant = String::new();
    for index in 0..2048 {
        let envelope = uuid::Uuid::new_v4().to_string();
        let result = uuid::Uuid::new_v4().to_string();
        let identifier = format!("call-{index}");
        if index == 0 {
            relevant.clone_from(&result);
        }
        history.push(entry(
            &envelope,
            Message::assistant_with_tools(vec![call(&identifier)]),
            false,
            true,
        ));
        history.push(entry(
            &result,
            Message::tool_result(&identifier, "Original durable evidence"),
            false,
            true,
        ));
        covered.extend([envelope, result]);
    }
    history.push(entry(
        "current",
        Message::user("Continue the actual request"),
        true,
        false,
    ));
    let mut state: Value = serde_json::from_str(&structured()).unwrap();
    state["evidence"] = json!([format!("Relevant tool fact; retrieve source {relevant}")]);
    let summary = Summary::new(
        state.to_string(),
        context::coverage(&history, &covered).unwrap(),
        4,
    );
    let prepared = context::prepare(
        &provider.completions(),
        "test",
        &history,
        None,
        &policy(4096),
        Some(&summary),
    )
    .await
    .unwrap();
    assert_eq!(prepared.summary.as_ref(), Some(&summary));
    assert_eq!(prepared.messages.len(), 3);
    assert!(
        prepared.messages[1]
            .content
            .as_ref()
            .unwrap()
            .contains(&relevant)
    );
    assert!(
        !prepared.messages[1]
            .content
            .as_ref()
            .unwrap()
            .contains(&covered[3])
    );
    assert!(prepared.usage.used < policy(4096).threshold().unwrap());
    assert!(provider.requests.lock().await.is_empty());
    context::validate(&history, prepared.summary.as_ref()).unwrap();
}
