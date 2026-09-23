//! Flattening a conversation into the single prompt a CLI agent accepts.

use std::fmt::Write;

use abnegate_llm::Message;
use abnegate_llm::Role;

const SYSTEM: &str = "System";
const USER: &str = "User";
const ASSISTANT: &str = "Assistant";
const TOOL: &str = "Tool result";

/// Render a conversation as one prompt.
///
/// A coding agent takes a prompt, not a message array, so the roles have to
/// survive as text. A bare concatenation loses who said what, and an agent
/// that cannot tell its own earlier reply from the user's instruction will
/// answer the wrong one.
pub fn render(messages: &[Message]) -> String {
    let mut prompt = String::new();

    for message in messages {
        let Some(content) = message.content.as_deref().map(str::trim) else {
            continue;
        };
        if content.is_empty() {
            continue;
        }
        let label = match message.role {
            Role::System => SYSTEM,
            Role::User => USER,
            Role::Assistant => ASSISTANT,
            Role::Tool => TOOL,
        };
        if !prompt.is_empty() {
            prompt.push_str("\n\n");
        }
        let _ = write!(prompt, "{label}:\n{content}");
    }

    prompt
}

#[cfg(test)]
mod tests {
    use abnegate_llm::FunctionCall;
    use abnegate_llm::Message;
    use abnegate_llm::ToolCall;

    use super::render;

    #[test]
    fn every_role_is_named_in_the_prompt() {
        let prompt = render(&[
            Message::system("Be terse."),
            Message::user("What does main do?"),
            Message::assistant("Let me look."),
            Message::tool_result("toolu_01", "fn main() {}"),
            Message::user("Thanks."),
        ]);

        assert_eq!(
            prompt,
            "System:\nBe terse.\n\nUser:\nWhat does main do?\n\nAssistant:\nLet me look.\n\nTool result:\nfn main() {}\n\nUser:\nThanks."
        );
    }

    #[test]
    fn a_message_carrying_only_tool_calls_contributes_nothing() {
        let prompt = render(&[
            Message::user("Read it."),
            Message::assistant_with_tools(vec![ToolCall {
                id: "toolu_01".to_string(),
                call_type: "function".to_string(),
                function: FunctionCall {
                    name: "read".to_string(),
                    arguments: "{}".to_string(),
                },
            }]),
        ]);

        assert_eq!(prompt, "User:\nRead it.");
    }

    #[test]
    fn blank_content_is_skipped_rather_than_padded() {
        let prompt = render(&[
            Message::system("   "),
            Message::user("  Only this.  "),
            Message::assistant(""),
        ]);

        assert_eq!(prompt, "User:\nOnly this.");
    }

    #[test]
    fn an_empty_conversation_renders_an_empty_prompt() {
        assert!(render(&[]).is_empty());
    }
}
