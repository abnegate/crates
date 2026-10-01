//! Heuristic lesson extraction from trial text.

use crate::lesson::Lesson;

const ROOT_MARKERS: [&str; 6] = [
    "root cause:",
    "root cause ",
    "failed because ",
    "skipped because ",
    "the issue was ",
    "the problem was ",
];

/// Heuristic lesson extraction from trial text.
///
/// Looks for root-cause phrases, skip/unavailable/blocked lines, and explicit
/// `avoid` lines. A host that has an LLM can replace this; the fallback needs
/// no model.
pub struct Extractor;

impl Extractor {
    /// Extract a lesson from `text`.
    pub fn extract(text: &str) -> Lesson {
        let mut lesson = Lesson::new();
        let mut seen_avoid = std::collections::BTreeSet::new();

        for line in text.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let lower = trimmed.to_ascii_lowercase();

            if lesson.root_cause.is_none()
                && let Some(rest) = after_marker(&lower, trimmed, &ROOT_MARKERS)
                && rest.len() > 3
            {
                lesson.root_cause = Some(rest);
            }

            if let Some(rest) = after_marker(&lower, trimmed, &["avoid "]) {
                if seen_avoid.insert(rest.clone()) {
                    lesson.avoid.push(rest);
                }
                continue;
            }

            if is_skip_line(&lower) && seen_avoid.insert(trimmed.to_string()) {
                lesson.avoid.push(trimmed.to_string());
            }
        }

        lesson
    }

    /// Compact one-line summary for storage. Empty when nothing was extracted.
    pub fn summarize(lesson: &Lesson) -> String {
        let mut parts = Vec::new();
        if let Some(root_cause) = &lesson.root_cause {
            parts.push(format!("root cause: {root_cause}"));
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
    fn extracts_root_cause_and_skip_reason() {
        let lesson = Extractor::extract(
            "fuzzilli=reprl-unavailable\nThe issue was the missing REPRL build.\n",
        );
        assert!(
            lesson
                .root_cause
                .as_deref()
                .is_some_and(|text| text.contains("REPRL"))
        );
        assert!(
            lesson
                .avoid
                .iter()
                .any(|line| line.contains("reprl-unavailable"))
        );
        let summary = Extractor::summarize(&lesson);
        assert!(summary.contains("root cause"));
        assert!(summary.contains("avoid"));
    }

    #[test]
    fn empty_text_is_an_empty_lesson() {
        let lesson = Extractor::extract("");
        assert!(lesson.is_empty());
        assert!(Extractor::summarize(&lesson).is_empty());
    }
}
