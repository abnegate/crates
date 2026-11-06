//! The line a reader is shown for a tool an agent reached for.

use abnegate_llm::ToolCall;
use abnegate_secret::sanitize;
use serde_json::Value;

const SUBJECTS: &[&str] = &[
    "command",
    "file_path",
    "pattern",
    "path",
    "query",
    "url",
    "description",
    "prompt",
];

const SUBJECT_LENGTH: usize = 80;

const ELLIPSIS: char = '\u{2026}';

const BACKTICK: char = '`';

/// The markdown list item a reader is shown for `call`: `- {name}` and, in
/// a code span, the first line of what the call was about, such as the
/// command it ran or the file it read, ending in a newline.
///
/// What the call was about is the first non-blank of its `command`,
/// `file_path`, `pattern`, `path`, `query`, `url`, `description` and
/// `prompt` arguments, sanitized of control sequences and credentials and
/// then cut to its first line and to 80 characters, with `…` marking a cut.
/// A call with none of them, or with arguments that are not JSON, is listed
/// by name alone.
///
/// A call whose name starts with one of the `hidden` prefixes, such as
/// `mcp__host__` for the tools of an MCP server the host renders itself, has
/// no line, and its arguments are never read.
///
/// ```
/// use abnegate_agent_cli::activity;
/// use abnegate_llm::ToolCall;
///
/// let call = ToolCall::function("toolu_01", "Bash", r#"{"command":"git status"}"#);
/// assert_eq!(activity(&call, &[]).as_deref(), Some("- Bash `git status`\n"));
///
/// let hidden = ToolCall::function("toolu_02", "mcp__host__search", "{}");
/// assert_eq!(activity(&hidden, &["mcp__host__"]), None);
/// ```
pub fn activity(call: &ToolCall, hidden: &[&str]) -> Option<String> {
    let name = &call.function.name;
    if hidden.iter().any(|prefix| name.starts_with(prefix)) {
        return None;
    }
    Some(match subject(&call.function.arguments) {
        Some(subject) => format!("- {name} {}\n", code_span(&subject)),
        None => format!("- {name}\n"),
    })
}

/// The subject is cut only once it is sanitized: a credential cut in half no
/// longer looks like one, and a carriage return left in would end the line
/// the code span has to stay on.
fn subject(arguments: &str) -> Option<String> {
    let arguments: Value = serde_json::from_str(arguments).ok()?;
    let text = SUBJECTS.iter().find_map(|key| {
        arguments
            .get(key)?
            .as_str()
            .map(str::trim)
            .filter(|text| !text.is_empty())
    })?;
    let text = sanitize(text);
    let line = text.lines().next()?.trim_end();
    let shown: String = line.chars().take(SUBJECT_LENGTH).collect();
    if shown.len() == text.len() {
        Some(shown)
    } else {
        Some(format!("{shown}{ELLIPSIS}"))
    }
}

/// `text` as inline code, fenced by one more backtick than the longest run
/// of them it holds, so that none of it can close the span.
fn code_span(text: &str) -> String {
    let longest = text
        .split(|character| character != BACKTICK)
        .map(str::len)
        .max()
        .unwrap_or(0);
    let fence = BACKTICK.to_string().repeat(longest + 1);
    let padding = if text.starts_with(BACKTICK) || text.ends_with(BACKTICK) {
        " "
    } else {
        ""
    };
    format!("{fence}{padding}{text}{padding}{fence}")
}

#[cfg(test)]
mod tests {
    use abnegate_llm::ToolCall;
    use serde_json::json;

    use super::SUBJECT_LENGTH;
    use super::activity;
    use super::code_span;

    const HIDDEN: &[&str] = &["mcp__host__"];

    fn call(name: &str, arguments: &str) -> ToolCall {
        ToolCall::function("toolu_01", name, arguments)
    }

    fn listed(name: &str, arguments: &str) -> Option<String> {
        activity(&call(name, arguments), HIDDEN)
    }

    #[test]
    fn an_agents_own_tool_is_listed_with_what_it_was_used_on() {
        assert_eq!(
            listed("Bash", r#"{"command":"git status"}"#).as_deref(),
            Some("- Bash `git status`\n")
        );
        assert_eq!(
            listed("ToolSearch", r#"{"query":"select:Read","max_results":1}"#).as_deref(),
            Some("- ToolSearch `select:Read`\n")
        );
        assert_eq!(
            listed("command_execution", r#"{"command":"/bin/zsh -lc ls"}"#).as_deref(),
            Some("- command_execution `/bin/zsh -lc ls`\n")
        );
    }

    #[test]
    fn a_tool_with_nothing_to_show_is_listed_by_name() {
        for arguments in ["{}", "not json", r#"{"command":"  "}"#, r#"{"todos":[]}"#] {
            assert_eq!(
                listed("TodoWrite", arguments).as_deref(),
                Some("- TodoWrite\n"),
                "{arguments}"
            );
        }
    }

    #[test]
    fn a_hidden_servers_tools_are_left_to_the_host() {
        assert_eq!(listed("mcp__host__search_code", "{}"), None);
    }

    /// The prefix is checked before the arguments are read, so a hidden call
    /// costs no parse however it was made.
    #[test]
    fn a_hidden_call_is_skipped_before_its_arguments_are_read() {
        assert_eq!(listed("mcp__host__search_code", "not json {"), None);
        assert_eq!(
            listed(
                "mcp__host__search_code",
                &json!({ "command": "git status" }).to_string()
            ),
            None
        );
    }

    #[test]
    fn another_servers_tool_is_not_mistaken_for_a_hidden_one() {
        assert_eq!(
            listed("mcp__hostish__search", "{}").as_deref(),
            Some("- mcp__hostish__search\n")
        );
    }

    #[test]
    fn a_long_or_multiline_subject_is_cut_short() {
        let long = "x".repeat(SUBJECT_LENGTH * 2);
        let shown = listed("Bash", &json!({ "command": long }).to_string()).expect("a line");
        assert_eq!(
            shown,
            format!("- Bash `{}\u{2026}`\n", "x".repeat(SUBJECT_LENGTH))
        );

        let script = "cd /w\ncargo test";
        assert_eq!(
            listed("Bash", &json!({ "command": script }).to_string()).as_deref(),
            Some("- Bash `cd /w\u{2026}`\n")
        );
    }

    #[test]
    fn a_secret_in_a_subject_is_redacted_before_it_is_cut() {
        let token = format!("ghp_{}", "a".repeat(36));
        let padded = format!(
            "{}export GITHUB_TOKEN={token}",
            "x".repeat(SUBJECT_LENGTH - 40)
        );
        let shown = listed("Bash", &json!({ "command": padded }).to_string()).expect("a line");

        assert!(!shown.contains("ghp_"), "{shown}");
    }

    #[test]
    fn a_carriage_return_cannot_end_the_line_a_subject_stays_on() {
        let shown = listed(
            "Bash",
            &json!({ "command": "ls\r# ![x](https://evil.example/)" }).to_string(),
        )
        .expect("a line");

        assert_eq!(shown, "- Bash `ls\u{2026}`\n");
    }

    #[test]
    fn a_subject_holding_backticks_cannot_close_its_span() {
        assert_eq!(code_span("echo `date`"), "`` echo `date` ``");
        assert_eq!(code_span("a ``b`` c"), "```a ``b`` c```");
        assert_eq!(code_span("plain"), "`plain`");
    }
}
