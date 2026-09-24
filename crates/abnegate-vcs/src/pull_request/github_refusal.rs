use crate::pull_request::service::summarised;
use serde_json::Value;

/// The field GitHub states a refusal in, on the answer and on each detail.
const MESSAGE: &str = "message";

/// The field GitHub lists a refusal's details under.
const ERRORS: &str = "errors";

/// What GitHub said in refusing a request: its message and each detail listed
/// under `errors`, and nothing else its answer carried.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct GitHubRefusal {
    message: Option<String>,
    errors: Vec<String>,
}

impl GitHubRefusal {
    /// GitHub's words in an error body. A detail is either an object carrying
    /// a message or a bare string; a body that is not GitHub's JSON says
    /// nothing.
    pub(super) fn parse(body: &[u8]) -> Self {
        let Ok(Value::Object(answer)) = serde_json::from_slice::<Value>(body) else {
            return Self::default();
        };
        Self {
            message: answer
                .get(MESSAGE)
                .and_then(Value::as_str)
                .map(str::to_string),
            errors: answer
                .get(ERRORS)
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(detail)
                .map(str::to_string)
                .collect(),
        }
    }

    /// Whether GitHub's message or any of its details contains `words`, in
    /// any ASCII case.
    pub(super) fn mentions(&self, words: &str) -> bool {
        let words = words.to_ascii_lowercase();
        self.said()
            .any(|said| said.to_ascii_lowercase().contains(&words))
    }

    /// GitHub's message and details on one line, sanitised, cut short once
    /// it grows too long to carry in an error.
    pub(super) fn summary(&self) -> String {
        summarised(self.said())
    }

    fn said(&self) -> impl Iterator<Item = &str> {
        self.message.iter().chain(&self.errors).map(String::as_str)
    }
}

fn detail(entry: &Value) -> Option<&str> {
    entry
        .as_str()
        .or_else(|| entry.get(MESSAGE).and_then(Value::as_str))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pull_request::service::bounded;

    #[test]
    fn github_s_words_are_found_in_its_message_and_in_every_error_detail() {
        let refusal = GitHubRefusal::parse(
            serde_json::json!({
                "message": "Validation Failed",
                "errors": [
                    {
                        "resource": "PullRequest",
                        "code": "custom",
                        "message": "A pull request already exists for acme:feature.",
                    },
                    "Reference does not exist",
                    { "resource": "Repository", "code": "missing_field" },
                ],
                "documentation_url": "https://docs.github.com/rest",
            })
            .to_string()
            .as_bytes(),
        );

        assert!(refusal.mentions("validation failed"));
        assert!(refusal.mentions("A PULL REQUEST ALREADY EXISTS"));
        assert!(refusal.mentions("reference does not exist"));
        assert!(
            !refusal.mentions("missing_field"),
            "a detail's code is not something GitHub said"
        );
        assert!(
            !refusal.mentions("docs.github.com"),
            "only the message and the details are GitHub's words"
        );

        let bare = GitHubRefusal::parse(br#"{"errors":["name already exists on this account"]}"#);
        assert!(bare.mentions("Already Exists"));

        for unspoken in [
            &b"<html>A pull request already exists</html>"[..],
            b"\"A pull request already exists\"",
            b"",
        ] {
            let silent = GitHubRefusal::parse(unspoken);
            assert!(!silent.mentions("already exists"), "{unspoken:?}");
            assert_eq!(silent, GitHubRefusal::default(), "{unspoken:?}");
        }
    }

    #[test]
    fn a_summary_is_github_s_words_on_one_line_and_only_so_long() {
        let refusal = GitHubRefusal::parse(
            serde_json::json!({
                "message": "Validation\nFailed",
                "errors": [
                    { "message": " name already exists " },
                    "   ",
                    { "code": "custom" },
                    "second\u{0}detail",
                ],
            })
            .to_string()
            .as_bytes(),
        );
        assert_eq!(
            refusal.summary(),
            "Validation Failed; name already exists; seconddetail"
        );

        assert_eq!(GitHubRefusal::default().summary(), "");

        let long = "é".repeat(4096);
        let summary = GitHubRefusal::parse(
            serde_json::json!({ "message": long })
                .to_string()
                .as_bytes(),
        )
        .summary();
        assert_eq!(summary, bounded(&long));
        assert!(summary.len() < long.len(), "{}", summary.len());
    }

    /// The redactor reads words a line break or a tab keeps apart as apart, so
    /// a summary keeps them apart too: dropping the break would join two
    /// halves into a credential the redactor never saw whole.
    #[test]
    fn words_the_redactor_read_apart_stay_apart() {
        let refusal = GitHubRefusal::parse(
            serde_json::json!({
                "message": "rejected ghp_\n0123456789abcdefghij",
                "errors": [
                    "ghp_\r\n0123456789abcdefghij",
                    "ghp_\u{2028}0123456789abcdefghij",
                    "ghp_\t \u{2029}0123456789abcdefghij",
                ],
            })
            .to_string()
            .as_bytes(),
        );

        assert_eq!(
            refusal.summary(),
            "rejected ghp_ 0123456789abcdefghij; ghp_ 0123456789abcdefghij; \
             ghp_ 0123456789abcdefghij; ghp_ 0123456789abcdefghij"
        );
    }

    #[test]
    fn a_summary_carries_nothing_that_could_hide_a_credential_or_forge_a_line() {
        let refusal = GitHubRefusal::parse(
            serde_json::json!({
                "message": "one\u{2028}two\u{2029}three",
                "errors": [
                    "a\u{202E}b\u{200B}c\u{2066}d\u{FEFF}",
                    "\u{1b}[31mred\u{1b}[0m",
                    concat!("rejected ghp_", "0123456789abcdefghij"),
                    "\u{200B}\u{2028}",
                ],
            })
            .to_string()
            .as_bytes(),
        );

        assert_eq!(
            refusal.summary(),
            "one two three; abcd; red; rejected [REDACTED]"
        );
    }
}
