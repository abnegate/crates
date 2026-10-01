//! Heuristic lesson extraction from trial text.

use crate::fingerprint::{diff_path, mentions_test, push_files};
use crate::lesson::Lesson;

const ROOT_MARKERS: [&str; 13] = [
    "the issue was ",
    "the issue is ",
    "the bug was ",
    "the bug is ",
    "the problem was ",
    "the problem is ",
    "the root cause was ",
    "the root cause is ",
    "root cause:",
    "root cause ",
    "fixed by ",
    "the fix was ",
    "the fix is ",
];

const DECISION_MARKERS: [&str; 4] = [
    "decided to ",
    "the decision was ",
    "chose to ",
    "went with ",
];

const ROOT_CAUSE_MINIMUM: usize = 11;
const ROOT_CAUSE_LIMIT: usize = 500;

/// Heuristic lesson extraction from trial text.
///
/// Looks for root-cause phrases, skip/unavailable/blocked lines, source paths,
/// diff markers, and test-run lines. A host that has an LLM can replace this;
/// the fallback needs no model.
pub struct Extractor;

impl Extractor {
    /// Extract a lesson from `text`.
    pub fn extract(text: &str) -> Lesson {
        let mut lesson = Lesson::new();
        let mut seen_avoid = std::collections::BTreeSet::new();
        let mut seen_decisions = std::collections::BTreeSet::new();
        let mut seen_files = std::collections::BTreeSet::new();
        let mut has_diff = false;

        for line in text.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let lower = trimmed.to_ascii_lowercase();

            if diff_path(trimmed).is_some() {
                has_diff = true;
            }
            push_files(trimmed, &mut lesson.files, &mut seen_files);

            if lesson.root_cause.is_none()
                && let Some(rest) = after_marker(&lower, trimmed, &ROOT_MARKERS)
                && rest.len() >= ROOT_CAUSE_MINIMUM
                && rest.len() < ROOT_CAUSE_LIMIT
            {
                lesson.root_cause = Some(rest);
            }

            if let Some(rest) = after_marker(&lower, trimmed, &["avoid "]) {
                if seen_avoid.insert(rest.clone()) {
                    lesson.avoid.push(rest);
                }
                continue;
            }

            if let Some(rest) = after_marker(&lower, trimmed, &DECISION_MARKERS)
                && rest.len() >= ROOT_CAUSE_MINIMUM
                && seen_decisions.insert(rest.clone())
            {
                lesson.decisions.push(rest);
            }

            if is_skip_line(&lower) && seen_avoid.insert(trimmed.to_string()) {
                lesson.avoid.push(trimmed.to_string());
            }

            if mentions_test(trimmed) {
                lesson.tests = true;
            }
        }

        lesson.approach = Some(if lesson.tests && has_diff {
            "test_driven".to_string()
        } else if has_diff {
            "direct_fix".to_string()
        } else if !lesson.files.is_empty() {
            "investigation_then_fix".to_string()
        } else {
            "unknown".to_string()
        });

        lesson
    }

    /// Compact one-line summary for storage. Empty when nothing was extracted.
    pub fn summarize(lesson: &Lesson) -> String {
        if lesson.is_empty() {
            return String::new();
        }
        let mut parts = Vec::new();
        if let Some(root_cause) = &lesson.root_cause {
            parts.push(format!("root cause: {root_cause}"));
        }
        if !lesson.files.is_empty() {
            let preview: Vec<&str> = lesson.files.iter().take(5).map(String::as_str).collect();
            parts.push(format!(
                "files ({}): {}",
                lesson.files.len(),
                preview.join(", ")
            ));
        }
        if let Some(approach) = &lesson.approach
            && approach != "unknown"
        {
            parts.push(format!("approach: {approach}"));
        }
        if lesson.tests {
            parts.push("tests were run".to_string());
        }
        if !lesson.avoid.is_empty() {
            parts.push(format!("avoid: {}", lesson.avoid.join("; ")));
        }
        if !lesson.decisions.is_empty() {
            parts.push(format!("decisions: {}", lesson.decisions.join("; ")));
        }
        parts.join(". ")
    }
}

fn after_marker(lower: &str, original: &str, markers: &[&str]) -> Option<String> {
    for marker in markers {
        if let Some(index) = lower.find(marker) {
            let rest = original[index + marker.len()..].trim();
            let rest = rest.trim_start_matches([':', '.', '-']).trim();
            if !rest.is_empty() {
                return Some(rest.to_string());
            }
        }
    }
    None
}

fn is_skip_line(lower: &str) -> bool {
    lower.contains("unavailable")
        || lower.contains("blocked")
        || lower.contains("-missing")
        || lower.contains("skipped:")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_root_cause_files_tests_and_skip_reason() {
        let lesson = Extractor::extract(
            "Reading file src/main.rs\n--- a/src/handler.rs\n+++ b/src/handler.rs\nThe issue was the missing null check in the handler.\nRunning cargo test\nAll tests passed.\nfuzzilli=reprl-unavailable\n",
        );
        assert!(
            lesson
                .root_cause
                .as_deref()
                .is_some_and(|text| text.contains("missing null check"))
        );
        assert!(lesson.files.contains(&"src/handler.rs".to_string()));
        assert!(lesson.files.contains(&"src/main.rs".to_string()));
        assert!(lesson.tests);
        assert_eq!(lesson.approach.as_deref(), Some("test_driven"));
        assert!(
            lesson
                .avoid
                .iter()
                .any(|line| line.contains("reprl-unavailable"))
        );
        let summary = Extractor::summarize(&lesson);
        assert!(summary.contains("root cause"));
        assert!(summary.contains("avoid"));
        assert!(summary.contains("files"));
    }

    #[test]
    fn extracts_named_decisions() {
        let lesson = Extractor::extract(
            "Decided to retry without the skipped strategy.\nChose to inspect the crash log first.\n",
        );
        assert!(
            lesson
                .decisions
                .iter()
                .any(|line| line.contains("retry without"))
        );
        assert!(
            lesson
                .decisions
                .iter()
                .any(|line| line.contains("inspect the crash"))
        );
        assert!(Extractor::summarize(&lesson).contains("decisions"));
    }

    #[test]
    fn empty_text_is_an_empty_lesson() {
        let lesson = Extractor::extract("");
        assert!(lesson.is_empty());
        assert!(Extractor::summarize(&lesson).is_empty());
    }

    #[test]
    fn short_root_cause_is_ignored_and_diff_without_tests_is_direct_fix() {
        let short = Extractor::extract("The issue was foo");
        assert!(short.root_cause.is_none());
        let direct = Extractor::extract("+++ b/src/handler.rs\n--- a/src/handler.rs");
        assert_eq!(direct.approach.as_deref(), Some("direct_fix"));
        assert!(direct.files.contains(&"src/handler.rs".to_string()));
    }
}
