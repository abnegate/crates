//! The report a coding agent returns when held to [`StructuredResult::SCHEMA`].

use serde::Deserialize;
use serde::Serialize;

use crate::question::BlockingQuestion;

/// A coding agent's own account of a task, shaped by `--json-schema`.
///
/// Constrained decoding guarantees the final answer matches
/// [`StructuredResult::SCHEMA`], which replaces scraping the agent's prose for
/// a pull request link or a question marker.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StructuredResult {
    #[serde(default)]
    pub summary: String,
    pub success: bool,
    #[serde(default)]
    pub pr_url: Option<String>,
    #[serde(default)]
    pub changelog: Option<String>,
    #[serde(default)]
    pub blocking_question: Option<BlockingQuestion>,
    /// How sure the agent is, from 0 to 100, that the change is correct and
    /// introduces no regressions.
    #[serde(default)]
    pub confidence: u8,
    #[serde(default)]
    pub confidence_reasoning: Option<String>,
    /// The repository the agent believes the task belongs in, in `org/repo`
    /// form, when it is not the one it was run in.
    #[serde(default, rename = "wrong_repo")]
    pub wrong_repository: Option<String>,
}

impl StructuredResult {
    /// The JSON schema passed to `claude --json-schema`.
    pub const SCHEMA: &str = r#"{
    "type": "object",
    "required": ["summary", "success", "confidence"],
    "additionalProperties": false,
    "properties": {
        "summary": {
            "type": "string",
            "description": "Brief summary of what was done or why you stopped"
        },
        "success": {
            "type": "boolean",
            "description": "Whether the task was completed successfully"
        },
        "pr_url": {
            "type": ["string", "null"],
            "description": "URL of the created pull request, if one was created"
        },
        "changelog": {
            "type": ["string", "null"],
            "description": "A succinct bullet-point list of the changes made (e.g. '- Fixed null check in auth handler\\n- Added unit test for edge case'). Null if no changes were made."
        },
        "blocking_question": {
            "type": ["object", "null"],
            "description": "If you need human input to proceed, provide the question here instead of attempting the task",
            "required": ["question"],
            "properties": {
                "question": { "type": "string" },
                "context": { "type": ["string", "null"] },
                "options": { "type": "array", "items": { "type": "string" } },
                "why": { "type": ["string", "null"] }
            },
            "additionalProperties": false
        },
        "confidence": {
            "type": "integer",
            "minimum": 0,
            "maximum": 100,
            "description": "Your confidence (0-100) that this change correctly completes the task without introducing regressions"
        },
        "confidence_reasoning": {
            "type": ["string", "null"],
            "description": "Brief explanation of your confidence level (e.g. what makes you certain or uncertain)"
        },
        "wrong_repo": {
            "type": ["string", "null"],
            "description": "If this is the wrong repository for the task, set this to the name of the correct repository in 'org/repo' format. Leave null if this is the correct repository."
        }
    }
}"#;

    /// Read a report out of a schema-shaped answer, or `None` when the
    /// answer does not satisfy this shape.
    pub fn from_output(output: &serde_json::Value) -> Option<Self> {
        Self::deserialize(output).ok()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::Value;
    use serde_json::json;

    use super::StructuredResult;

    fn parse(json: &str) -> StructuredResult {
        serde_json::from_str(json).expect("a structured result")
    }

    fn schema() -> Value {
        serde_json::from_str(StructuredResult::SCHEMA).expect("the schema to be JSON")
    }

    fn confidence(output: Option<Value>) -> (u8, Option<String>) {
        output
            .as_ref()
            .and_then(StructuredResult::from_output)
            .map(|result| (result.confidence, result.confidence_reasoning))
            .unwrap_or((0, None))
    }

    #[test]
    fn a_full_report_is_read() {
        let result = parse(
            r#"{"summary":"Fixed the bug and created PR","success":true,"pr_url":"https://github.com/org/repo/pull/42","blocking_question":null}"#,
        );
        assert!(result.success);
        assert_eq!(result.summary, "Fixed the bug and created PR");
        assert_eq!(
            result.pr_url.as_deref(),
            Some("https://github.com/org/repo/pull/42")
        );
        assert!(result.blocking_question.is_none());
    }

    #[test]
    fn a_report_can_carry_a_blocking_question() {
        let result = parse(
            r#"{"summary":"Need clarification","success":false,"pr_url":null,"blocking_question":{"question":"Which branch?","context":"Multiple candidates","options":["main","develop"],"why":"Ambiguous target"}}"#,
        );
        assert!(!result.success);
        assert!(result.pr_url.is_none());
        let question = result.blocking_question.expect("a question");
        assert_eq!(question.question, "Which branch?");
        assert_eq!(question.context.as_deref(), Some("Multiple candidates"));
        assert_eq!(question.options, vec!["main", "develop"]);
        assert_eq!(question.why.as_deref(), Some("Ambiguous target"));
    }

    #[test]
    fn a_minimal_report_defaults_everything_else() {
        let result = parse(r#"{"summary":"Done","success":true}"#);
        assert!(result.success);
        assert_eq!(result.summary, "Done");
        assert!(result.pr_url.is_none());
        assert!(result.changelog.is_none());
        assert!(result.blocking_question.is_none());
        assert_eq!(result.confidence, 0);
        assert!(result.confidence_reasoning.is_none());
        assert!(result.wrong_repository.is_none());
    }

    #[test]
    fn an_empty_summary_survives() {
        let result = parse(r#"{"summary":"","success":false}"#);
        assert!(!result.success);
        assert!(result.summary.is_empty());
    }

    #[test]
    fn a_link_is_kept_verbatim_for_the_caller_to_vet() {
        for link in ["http://github.com/org/repo/pull/1", "", "not-a-url"] {
            let json = json!({"summary": "done", "success": true, "pr_url": link});
            let result = StructuredResult::from_output(&json).expect("a report");
            assert_eq!(result.pr_url.as_deref(), Some(link));
        }
    }

    #[test]
    fn a_minimal_or_empty_question_is_accepted() {
        let result = parse(
            r#"{"summary":"stuck","success":false,"blocking_question":{"question":"help?"}}"#,
        );
        let question = result.blocking_question.expect("a question");
        assert_eq!(question.question, "help?");
        assert!(question.context.is_none());
        assert!(question.options.is_empty());
        assert!(question.why.is_none());

        let result = parse(
            r#"{"summary":"stuck","success":false,"blocking_question":{"question":"q","context":null,"options":[],"why":null}}"#,
        );
        assert!(
            result
                .blocking_question
                .expect("a question")
                .options
                .is_empty()
        );

        let result = parse(
            r#"{"summary":"choices","success":false,"blocking_question":{"question":"Pick one","options":["a","b","c","d","e","f","g","h"]}}"#,
        );
        assert_eq!(
            result.blocking_question.expect("a question").options.len(),
            8
        );
    }

    #[test]
    fn a_report_without_success_is_not_a_report() {
        for output in [
            json!({ "not_summary": "bad", "not_success": true }),
            json!({ "summary": "done" }),
            json!("not an object"),
        ] {
            assert!(StructuredResult::from_output(&output).is_none(), "{output}");
        }
    }

    #[test]
    fn a_report_is_read_from_a_json_value() {
        let result = StructuredResult::from_output(&json!({
            "summary": "Stuck",
            "success": false,
            "pr_url": null,
            "changelog": "- Fixed the bug\n- Updated tests",
            "blocking_question": {
                "question": "Which database should I use?",
                "context": "Found multiple database configs",
                "options": ["postgres", "mysql"],
                "why": "Ambiguous configuration"
            }
        }))
        .expect("a report");
        assert!(!result.success);
        assert_eq!(
            result.changelog.as_deref(),
            Some("- Fixed the bug\n- Updated tests")
        );
        let question = result.blocking_question.expect("a question");
        assert_eq!(question.question, "Which database should I use?");
        assert_eq!(question.options.len(), 2);
    }

    #[test]
    fn a_changelog_is_kept_as_written() {
        assert_eq!(
            parse(r#"{"summary":"Applied fix","success":true,"changelog":"- Fixed null check\n- Added test"}"#)
                .changelog
                .as_deref(),
            Some("- Fixed null check\n- Added test")
        );
        assert!(
            parse(r#"{"summary":"Done","success":true,"changelog":null}"#)
                .changelog
                .is_none()
        );
        assert_eq!(
            parse(r#"{"summary":"Done","success":true,"changelog":""}"#)
                .changelog
                .as_deref(),
            Some("")
        );
        assert_eq!(
            parse(r#"{"summary":"Done","success":true,"changelog":"   "}"#)
                .changelog
                .as_deref(),
            Some("   ")
        );
    }

    #[test]
    fn confidence_and_its_reasoning_are_read() {
        let result = parse(
            r#"{"summary":"Fixed auth bug","success":true,"pr_url":"https://github.com/org/repo/pull/10","confidence":85,"confidence_reasoning":"Tests pass and the fix is localized"}"#,
        );
        assert_eq!(result.confidence, 85);
        assert_eq!(
            result.confidence_reasoning.as_deref(),
            Some("Tests pass and the fix is localized")
        );

        for (json, expected) in [
            (r#"{"summary":"No idea","success":false,"confidence":0}"#, 0),
            (
                r#"{"summary":"Certain","success":true,"confidence":100,"confidence_reasoning":"Exact same bug pattern seen before"}"#,
                100,
            ),
            (
                r#"{"summary":"Done","success":true,"confidence":50,"confidence_reasoning":null}"#,
                50,
            ),
        ] {
            assert_eq!(parse(json).confidence, expected, "{json}");
        }

        assert_eq!(
            parse(r#"{"summary":"Done","success":true,"confidence":60,"confidence_reasoning":""}"#)
                .confidence_reasoning
                .as_deref(),
            Some("")
        );
        assert!(
            parse(r#"{"summary":"Fixed","success":true,"confidence":75,"confidence_reasoning":"Fix is 确定的 — tests pass ✓"}"#)
                .confidence_reasoning
                .expect("a reasoning")
                .contains("确定的")
        );
    }

    #[test]
    fn a_confidence_outside_a_byte_rejects_the_whole_report() {
        for value in [json!(85.7), json!(300), json!(-5)] {
            let output = json!({"summary": "Fixed", "success": true, "confidence": value});
            assert_eq!(confidence(Some(output)), (0, None), "{value}");
        }
        assert_eq!(confidence(None), (0, None));
        assert_eq!(confidence(Some(json!("not an object"))), (0, None));
        assert_eq!(
            confidence(Some(json!({
                "summary": "Fixed",
                "success": true,
                "confidence": 92,
                "confidence_reasoning": "All tests pass"
            }))),
            (92, Some("All tests pass".to_string()))
        );
    }

    #[test]
    fn a_wrong_repository_is_read_from_its_wire_name() {
        let result = parse(r#"{"summary":"Wrong place","success":false,"wrong_repo":"org/other"}"#);
        assert_eq!(result.wrong_repository.as_deref(), Some("org/other"));

        let json = serde_json::to_value(&result).expect("serialisable");
        assert_eq!(json["wrong_repo"], "org/other");
    }

    #[test]
    fn every_field_together_is_read_with_the_question() {
        let result = parse(
            r#"{
                "summary": "Need info",
                "success": false,
                "pr_url": "https://github.com/org/repo/pull/10",
                "changelog": "- Fixed null check\n- Added test",
                "confidence": 0,
                "confidence_reasoning": "Cannot proceed without human input",
                "blocking_question": {"question": "Which database?", "options": ["postgres", "mysql"]}
            }"#,
        );
        assert!(!result.success);
        assert_eq!(result.confidence, 0);
        assert!(result.pr_url.is_some());
        assert!(result.changelog.is_some());
        assert!(result.blocking_question.is_some());
        assert_eq!(
            result.confidence_reasoning.as_deref(),
            Some("Cannot proceed without human input")
        );
    }

    #[test]
    fn debug_names_the_type_and_its_summary() {
        let result = parse(r#"{"summary":"test","success":true}"#);
        let debug = format!("{result:?}");
        assert!(debug.contains("StructuredResult"));
        assert!(debug.contains("test"));
    }

    #[test]
    fn the_schema_is_a_closed_object_with_the_required_fields() {
        let schema = schema();
        assert_eq!(schema["type"], "object");
        assert_eq!(schema["additionalProperties"], false);

        let required: Vec<&str> = schema["required"]
            .as_array()
            .expect("a required list")
            .iter()
            .filter_map(Value::as_str)
            .collect();
        assert_eq!(required, ["summary", "success", "confidence"]);

        let properties = schema["properties"].as_object().expect("properties");
        for key in [
            "summary",
            "success",
            "pr_url",
            "changelog",
            "blocking_question",
            "confidence",
            "confidence_reasoning",
            "wrong_repo",
        ] {
            assert!(properties.contains_key(key), "{key} is missing");
        }
        assert_eq!(schema["properties"]["summary"]["type"], "string");
        assert_eq!(schema["properties"]["success"]["type"], "boolean");
    }

    #[test]
    fn nullable_fields_allow_null() {
        let schema = schema();
        for key in ["pr_url", "changelog", "blocking_question", "wrong_repo"] {
            let types: Vec<&str> = schema["properties"][key]["type"]
                .as_array()
                .expect("a type list")
                .iter()
                .filter_map(Value::as_str)
                .collect();
            assert!(types.contains(&"null"), "{key} does not allow null");
        }
        assert!(
            !schema["properties"]["changelog"]["description"]
                .as_str()
                .expect("a description")
                .is_empty()
        );
    }

    #[test]
    fn the_question_sub_schema_names_every_field() {
        let schema = schema();
        let question = &schema["properties"]["blocking_question"];
        assert_eq!(question["required"], json!(["question"]));
        let properties = question["properties"].as_object().expect("properties");
        for key in ["question", "context", "options", "why"] {
            assert!(properties.contains_key(key), "{key} is missing");
        }
    }

    #[test]
    fn confidence_is_a_bounded_integer_so_decoding_never_produces_a_fraction() {
        let schema = schema();
        let confidence = &schema["properties"]["confidence"];
        assert_eq!(confidence["type"], "integer");
        assert_eq!(confidence["minimum"], 0);
        assert_eq!(confidence["maximum"], 100);
        assert!(
            confidence["description"]
                .as_str()
                .expect("a description")
                .contains("0-100")
        );
        assert!(schema["properties"]["confidence_reasoning"].is_object());
    }
}
