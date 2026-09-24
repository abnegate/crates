use abnegate_agent::chat::Capacity;
use abnegate_agent::chat::Entry;
use abnegate_agent::chat::Evidence;
use abnegate_agent::chat::History;
use abnegate_agent::chat::Lease;
use abnegate_agent::chat::NewEntry;
use abnegate_agent::chat::ReplayMessage;
use abnegate_agent::chat::Source;
use abnegate_agent::chat::StoredMessage;
use abnegate_agent::chat::Summary;
use abnegate_agent::chat::fingerprint;
use abnegate_agent::chat::validate;
use abnegate_llm::Message;
use abnegate_llm::ToolCall;
use chrono::DateTime;
use chrono::Utc;
use serde_json::json;
use uuid::Uuid;

fn entry(id: &str, message: &Message) -> Entry {
    Entry::new(id, ReplayMessage::from(message))
}

/// A turn that read a file, and the request that followed it.
fn conversation() -> Vec<Entry> {
    let call = ToolCall::function("call_1", "read_file", "{}");
    vec![
        entry("user-1", &Message::user("Read the manifest.")),
        entry("assistant-1", &Message::assistant_with_tools(vec![call])).with_consumed(true),
        entry("tool-1", &Message::tool_result("call_1", "[workspace]")).with_consumed(true),
        entry("user-2", &Message::user("And now?")),
    ]
}

/// A store outside this crate builds what it loads, and the checkpoint it
/// read back is checked against it as any other would be.
#[test]
fn a_history_a_store_builds_is_validated_like_any_other() {
    let covered = vec!["assistant-1".to_string(), "tool-1".to_string()];
    let summary = Summary::new(
        "The manifest was read.",
        covered.clone(),
        fingerprint(&conversation(), &covered).unwrap(),
        1,
    );
    let history = History::new(conversation())
        .with_summary(summary.clone())
        .with_latest_user("user-2");

    assert_eq!(history.summary.as_ref(), Some(&summary));
    assert_eq!(history.latest_user.as_deref(), Some("user-2"));
    assert_eq!(validate(&history, &summary), Ok(()));

    let unconsumed = History::new(
        conversation()
            .into_iter()
            .map(|entry| entry.with_consumed(false))
            .collect(),
    );
    assert!(validate(&unconsumed, &summary).is_err());
}

#[test]
fn a_history_is_whole_unless_the_store_says_otherwise() {
    assert!(!History::new(conversation()).incomplete);
    assert!(
        History::new(conversation())
            .with_incomplete(true)
            .incomplete
    );
}

#[test]
fn a_store_builds_the_lease_and_the_messages_it_hands_back() {
    let chat = Uuid::new_v4();
    let owner = Uuid::new_v4();
    let expires = DateTime::<Utc>::from_timestamp(1_800_000_000, 0).unwrap();

    let lease = Lease::new(chat, owner, 7, expires);
    assert_eq!(
        (lease.chat_id, lease.owner, lease.fence, lease.expires_at),
        (chat, owner, 7, expires)
    );

    let id = Uuid::new_v4();
    let bare = StoredMessage::new(id, chat, "user", "Hello");
    assert_eq!((bare.id, bare.chat_id), (id, chat));
    assert_eq!(
        (bare.role.as_str(), bare.content.as_str()),
        ("user", "Hello")
    );
    assert!(bare.metadata.is_none());
    assert!(bare.created_at.is_none());
    assert!(!bare.title_claimed);

    let created = expires.naive_utc();
    let full = StoredMessage::new(id, chat, "user", "Hello")
        .with_metadata(json!({"source": "web"}))
        .with_created_at(created)
        .with_title_claimed(true);
    assert_eq!(full.metadata, Some(json!({"source": "web"})));
    assert_eq!(full.created_at, Some(created));
    assert!(full.title_claimed);
}

#[test]
fn a_new_entry_names_the_calls_recovery_must_never_retry() {
    let message = ReplayMessage::from(&Message::assistant("Deploying."));

    let quiet = NewEntry::new("assistant-1", message.clone());
    assert_eq!(quiet.id, "assistant-1");
    assert!(quiet.mutations.is_empty());

    let mutating = NewEntry::new("assistant-1", message).with_mutations(["call_1", "call_2"]);
    assert_eq!(mutating.mutations, ["call_1", "call_2"]);
}

#[test]
fn an_evidence_page_says_where_the_next_one_starts() {
    let last = Evidence::new("tool-1", "tail", 96, 100);
    assert_eq!(
        (
            last.id.as_str(),
            last.content.as_str(),
            last.offset,
            last.total
        ),
        ("tool-1", "tail", 96, 100)
    );
    assert!(last.next.is_none());

    let first = Evidence::new("tool-1", "head", 0, 100).with_next(4);
    assert_eq!(first.next, Some(4));
}

#[test]
fn a_capacity_carries_what_the_deployment_reported() {
    let reported = Capacity::new("openai/remote", Some(128_000), Source::Provider);
    assert_eq!(reported.identity, "openai/remote");
    assert_eq!(reported.limit, Some(128_000));
    assert_eq!(reported.source, Source::Provider);
    assert!(reported.ollama.is_none());
    assert!(!reported.reasoning);
    assert!(reported.reason.is_none());

    let local = Capacity::new("ollama/qwen3", Some(32_768), Source::Runtime)
        .with_ollama(32_768)
        .with_reasoning(true)
        .with_reason("the runtime reported its loaded context");
    assert_eq!(local.ollama, Some(32_768));
    assert!(local.reasoning);
    assert_eq!(
        local.reason.as_deref(),
        Some("the runtime reported its loaded context")
    );
}
