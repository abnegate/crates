use abnegate_llm::{Message, Role};
use std::fmt::Write;

use super::estimate::message_cost;

const END: &str = "<|end|>";

/// Room for the tags and the history around the context itself.
const PROMPT_HEADROOM: usize = 2048;

fn tag(role: Role) -> &'static str {
    match role {
        Role::System => "<|system|>",
        Role::User => "<|user|>",
        Role::Assistant => "<|assistant|>",
        Role::Tool => "<|tool|>",
    }
}

/// Render one prompt for a model that takes a flat, tagged transcript rather
/// than a list of chat messages.
///
/// The system section carries `system` and, when there is any, the retrieved
/// `context` beneath it. `history` follows in order, then `user_message`, and
/// the prompt ends on an open assistant tag for the model to complete.
pub fn build_chat_prompt(
    system: &str,
    context: &str,
    history: &[Message],
    user_message: &str,
) -> String {
    let mut prompt = String::with_capacity(system.len() + context.len() + PROMPT_HEADROOM);

    let _ = write!(prompt, "{}\n{system}", tag(Role::System));
    if !context.is_empty() {
        let _ = write!(prompt, "\n\n{context}");
    }
    let _ = writeln!(prompt, "\n{END}");

    for message in history {
        let _ = writeln!(
            prompt,
            "{}\n{}\n{END}",
            tag(message.role),
            message.content.as_deref().unwrap_or_default()
        );
    }

    let _ = write!(
        prompt,
        "{}\n{user_message}\n{END}\n{}\n",
        tag(Role::User),
        tag(Role::Assistant)
    );
    prompt
}

/// The most recent part of `history` whose estimated cost fits in `budget`.
///
/// Messages are counted back from the newest, so what is dropped is always the
/// oldest. The kept part never opens on a tool result: one cut away from the
/// call that asked for it answers nothing, and a provider rejects it.
pub fn trim_history(history: &[Message], budget: u64) -> &[Message] {
    let mut spent: u64 = 0;
    let mut start = history.len();
    for (index, message) in history.iter().enumerate().rev() {
        let (content, overhead) = message_cost(message);
        let cost = content.saturating_add(overhead);
        if spent.saturating_add(cost) > budget {
            break;
        }
        spent = spent.saturating_add(cost);
        start = index;
    }
    while history
        .get(start)
        .is_some_and(|message| message.role == Role::Tool)
    {
        start += 1;
    }
    &history[start..]
}

#[cfg(test)]
mod tests {
    use super::*;
    use abnegate_llm::{FunctionCall, ToolCall};

    const SYSTEM: &str = "\
You are a code assistant. Answer questions about the codebase using the provided code context.
Be precise and reference specific files, functions, and line numbers.
If the code context doesn't contain enough information to answer the question, say so.
Format code references as `file_path:line_number`.";

    #[test]
    fn test_build_prompt_empty_context() {
        let prompt = build_chat_prompt(SYSTEM, "", &[], "Hello");
        assert!(prompt.contains(SYSTEM));
        assert!(prompt.starts_with(&format!("<|system|>\n{SYSTEM}\n<|end|>\n")));
        assert!(prompt.contains("<|user|>\nHello\n"));
        assert!(prompt.ends_with("<|assistant|>\n"));
    }

    #[test]
    fn test_build_prompt_with_context_and_history() {
        let history = vec![
            Message::user("What is this?"),
            Message::assistant("It's a Rust project."),
        ];
        let prompt = build_chat_prompt(
            SYSTEM,
            "## Code\n```rust\nfn main() {}\n```",
            &history,
            "Tell me more",
        );

        assert!(prompt.contains(&format!("{SYSTEM}\n\n## Code")));
        assert!(prompt.contains("<|user|>\nWhat is this?"));
        assert!(prompt.contains("<|assistant|>\nIt's a Rust project."));
        assert!(prompt.contains("<|user|>\nTell me more"));
    }

    #[test]
    fn test_trim_history_empty() {
        assert!(trim_history(&[], 1000).is_empty());
    }

    #[test]
    fn test_trim_history_fits() {
        let history = vec![Message::user("Hi"), Message::assistant("Hello!")];
        let trimmed = trim_history(&history, 10000);
        assert_eq!(trimmed.len(), 2);
    }

    #[test]
    fn test_trim_history_overflow() {
        let long = "x".repeat(10000);
        let history = vec![
            Message::user(&long),
            Message::assistant(&long),
            Message::user("short"),
            Message::assistant("also short"),
        ];
        let trimmed = trim_history(&history, 100);
        assert_eq!(trimmed.len(), 2);
        assert_eq!(trimmed[0].content.as_deref(), Some("short"));
    }

    /// Cutting between a call and its results would leave the results
    /// answering nothing, so the cut moves forward past them instead.
    #[test]
    fn a_trimmed_history_never_opens_on_an_orphaned_tool_result() {
        let call = ToolCall {
            id: "call".to_string(),
            call_type: "function".to_string(),
            function: FunctionCall {
                name: "read_file".to_string(),
                arguments: "x".repeat(4000),
            },
        };
        let history = vec![
            Message::user("Read it"),
            Message::assistant_with_tools(vec![call]),
            Message::tool_result("call", "contents"),
            Message::assistant("Done"),
        ];

        let trimmed = trim_history(&history, 40);

        assert_eq!(trimmed.len(), 1, "{trimmed:?}");
        assert_eq!(trimmed[0].content.as_deref(), Some("Done"));
    }
}
