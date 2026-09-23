//! A question an agent stopped to ask rather than guess at.

use serde::Deserialize;
use serde::Serialize;

/// A question the agent needs a human to answer before it can go on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockingQuestion {
    pub question: String,
    /// What the agent found that made the question necessary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    /// The answers the agent proposes, when it has candidates.
    #[serde(default)]
    pub options: Vec<String>,
    /// Why the agent could not decide on its own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
}

impl BlockingQuestion {
    /// The first question found in `output` as a line of the form
    /// `<marker> {"question": ...}`.
    ///
    /// This is the fallback for a run that produced no schema-shaped answer,
    /// such as one killed part way through, where the agent was instructed to
    /// print its question on a line of its own behind `marker`. A line whose
    /// JSON does not parse is skipped rather than ending the search.
    pub fn extract(output: &str, marker: &str) -> Option<Self> {
        output.lines().find_map(|line| {
            let payload = line.trim().strip_prefix(marker)?.trim();
            if payload.is_empty() {
                return None;
            }
            serde_json::from_str::<Self>(payload).ok()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::BlockingQuestion;

    const MARKER: &str = "AGENT_QUESTION:";

    fn extract(output: &str) -> Option<BlockingQuestion> {
        BlockingQuestion::extract(output, MARKER)
    }

    #[test]
    fn a_full_question_is_read_from_among_other_output() {
        let output = "some logs\nAGENT_QUESTION: {\"question\":\"Which branch?\",\"context\":\"unclear\",\"options\":[\"main\",\"develop\"],\"why\":\"need branch\"}\ndone";

        let question = extract(output).expect("a question");
        assert_eq!(question.question, "Which branch?");
        assert_eq!(question.context.as_deref(), Some("unclear"));
        assert_eq!(question.options, vec!["main", "develop"]);
        assert_eq!(question.why.as_deref(), Some("need branch"));
    }

    #[test]
    fn only_the_question_itself_is_required() {
        for output in [
            r#"AGENT_QUESTION: {"question":"minimal"}"#,
            r#"AGENT_QUESTION: {"question":"minimal","context":null,"options":[],"why":null}"#,
        ] {
            let question = extract(output).expect("a question");
            assert_eq!(question.question, "minimal");
            assert!(question.context.is_none());
            assert!(question.options.is_empty());
            assert!(question.why.is_none());
        }
    }

    #[test]
    fn malformed_or_empty_payloads_yield_nothing() {
        for output in [
            "AGENT_QUESTION: {not valid json}",
            "AGENT_QUESTION: not valid json {{{",
            "AGENT_QUESTION:   ",
            "AGENT_QUESTION: ",
            "AGENT_QUESTION:",
            "",
        ] {
            assert!(extract(output).is_none(), "{output:?}");
        }
    }

    #[test]
    fn output_without_the_marker_yields_nothing() {
        for output in [
            "Just some regular output\nwithout any question markers\n",
            "just some regular output",
            "AGENT_QUESTION {\"question\":\"test\"}",
        ] {
            assert!(extract(output).is_none(), "{output:?}");
        }
    }

    #[test]
    fn the_first_of_several_questions_wins() {
        let output =
            "AGENT_QUESTION: {\"question\":\"first\"}\nAGENT_QUESTION: {\"question\":\"second\"}";
        assert_eq!(extract(output).expect("a question").question, "first");
    }

    #[test]
    fn a_malformed_line_does_not_hide_a_later_valid_one() {
        let output = "AGENT_QUESTION: {broken\nAGENT_QUESTION: {\"question\":\"second\"}";
        assert_eq!(extract(output).expect("a question").question, "second");
    }

    #[test]
    fn surrounding_whitespace_is_ignored() {
        let output = "   AGENT_QUESTION:  {\"question\":\"trimmed\"}  ";
        assert_eq!(extract(output).expect("a question").question, "trimmed");
    }

    #[test]
    fn the_question_is_found_among_many_lines() {
        for output in [
            "line 1\nline 2\nAGENT_QUESTION: {\"question\":\"How?\"}\nline 4",
            "INFO: Starting fix...\nDEBUG: Analyzing code\nAGENT_QUESTION: {\"question\":\"How?\"}\nINFO: Waiting for response",
        ] {
            assert_eq!(extract(output).expect("a question").question, "How?");
        }
    }

    #[test]
    fn every_field_is_read() {
        let output = r#"AGENT_QUESTION: {"question":"Which DB?","context":"Found postgres and mysql","options":["postgres","mysql","sqlite"],"why":"Cannot determine from config"}"#;

        let question = extract(output).expect("a question");
        assert_eq!(question.question, "Which DB?");
        assert_eq!(
            question.context.as_deref(),
            Some("Found postgres and mysql")
        );
        assert_eq!(question.options, vec!["postgres", "mysql", "sqlite"]);
        assert_eq!(
            question.why.as_deref(),
            Some("Cannot determine from config")
        );
    }

    #[test]
    fn unknown_fields_empty_questions_and_unicode_are_tolerated() {
        let question = extract(
            r#"AGENT_QUESTION: {"question":"test?","extra_field":"ignored","another":123}"#,
        )
        .expect("a question");
        assert_eq!(question.question, "test?");

        let question = extract(r#"AGENT_QUESTION: {"question":""}"#).expect("a question");
        assert!(question.question.is_empty());

        let question =
            extract(r#"AGENT_QUESTION: {"question":"Which 日本語 module?"}"#).expect("a question");
        assert_eq!(question.question, "Which 日本語 module?");
    }

    #[test]
    fn a_question_round_trips_without_its_absent_fields() {
        let question = BlockingQuestion {
            question: "Which branch?".to_string(),
            context: None,
            options: vec!["main".to_string()],
            why: None,
        };

        let json = serde_json::to_value(&question).expect("serialisable");
        assert!(json.get("context").is_none());
        assert!(json.get("why").is_none());
        assert_eq!(
            serde_json::from_value::<BlockingQuestion>(json).expect("deserialisable"),
            question
        );
    }
}
