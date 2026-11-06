use crate::pull_request::service::excerpted;
use crate::pull_request::service::summarised;
use serde_json::Value;

/// The field GitHub states a refusal in, on the answer and on each detail.
const MESSAGE: &str = "message";

/// The field GitHub lists a refusal's details under.
const ERRORS: &str = "errors";

/// What GitHub said in refusing a request: its message and each detail listed
/// under `errors`, and, only where its answer carried neither, the start of
/// that answer.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct GitHubRefusal {
    message: Option<String>,
    errors: Vec<String>,
    excerpt: String,
}

impl GitHubRefusal {
    /// GitHub's words in an error body. A detail is either an object carrying
    /// a message or a bare string. A body that is not GitHub's JSON, or that
    /// carries neither a message nor a detail, says nothing, and only its
    /// start is kept, as [`excerpted`] cuts it.
    pub(super) fn parse(body: &[u8]) -> Self {
        let Ok(Value::Object(answer)) = serde_json::from_slice::<Value>(body) else {
            return Self::unspoken(body);
        };
        let message = answer
            .get(MESSAGE)
            .and_then(Value::as_str)
            .map(str::to_string);
        let errors: Vec<String> = answer
            .get(ERRORS)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(detail)
            .map(str::to_string)
            .collect();
        match message.is_none() && errors.is_empty() {
            true => Self::unspoken(body),
            false => Self {
                message,
                errors,
                excerpt: String::new(),
            },
        }
    }

    /// A refusal whose `body` carried no words of GitHub's, kept only as the
    /// start of it.
    fn unspoken(body: &[u8]) -> Self {
        Self {
            excerpt: excerpted(body),
            ..Self::default()
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

    /// What an error carries of the refusal: its [`Self::summary`], or, where
    /// GitHub gave neither a message nor a detail, the start of its answer.
    pub(super) fn words(&self) -> String {
        let summary = self.summary();
        match summary.is_empty() {
            true => self.excerpt.clone(),
            false => summary,
        }
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
            br#"{"documentation_url":"A pull request already exists"}"#,
            b"",
        ] {
            let silent = GitHubRefusal::parse(unspoken);
            assert!(!silent.mentions("already exists"), "{unspoken:?}");
            assert_eq!(silent.summary(), "", "{unspoken:?}");
        }
    }

    #[test]
    fn an_answer_without_github_s_words_is_carried_as_its_start_alone() {
        for (body, words) in [
            (
                &br#"{"message":"Server Error","documentation_url":"https://docs"}"#[..],
                "Server Error",
            ),
            (
                br#"{"errors":["name already exists"]}"#,
                "name already exists",
            ),
            (br#"{"message":""}"#, ""),
            (
                b"<html>\n  <body>Bad gateway</body>\n</html>",
                "<html> <body>Bad gateway</body> </html>",
            ),
            (
                br#"{"documentation_url":"https://docs"}"#,
                r#"{"documentation_url":"https://docs"}"#,
            ),
            (b"\xff\xfeunreadable", "\u{FFFD}\u{FFFD}unreadable"),
            (b"", ""),
        ] {
            assert_eq!(GitHubRefusal::parse(body).words(), words, "{body:?}");
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
