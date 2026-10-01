//! Strategy fingerprint parsed from attempt text.

use std::collections::BTreeMap;

const FILE_PREFIXES: [&str; 8] = [
    "src/",
    "lib/",
    "app/",
    "pkg/",
    "internal/",
    "cmd/",
    "tests/",
    "test/",
];

const TEST_MARKERS: [&str; 5] = ["cargo test", "npm test", "pytest", "make test", "jest"];

/// How an attempt was approached, parsed from its log or summary.
///
/// Action names are supplied by the host so this crate never names a vendor
/// tool. Test-run markers and source-path prefixes are generic.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub struct Fingerprint {
    /// Named approach: `tdd`, `investigation`, `direct_fix`, `exploration`, or
    /// `unknown`.
    pub approach: String,
    /// Source paths mentioned in the text, first seen first.
    pub files: Vec<String>,
    /// How often each host-supplied action name appeared.
    pub actions: BTreeMap<String, i64>,
    /// How many test-run lines were seen.
    pub tests: i64,
    /// One-line summary of files, tests, and approach.
    pub summary: String,
}

impl Fingerprint {
    /// Parse `text`, counting each name in `actions` when it appears on a line.
    pub fn parse(text: &str, actions: &[&str]) -> Self {
        let mut files = Vec::new();
        let mut seen = std::collections::BTreeSet::new();
        let mut counts = BTreeMap::<String, i64>::new();
        let mut tests = 0_i64;
        let mut reads = 0_i64;
        let mut edits = 0_i64;

        for line in text.lines() {
            for action in actions {
                if line.contains(action) {
                    *counts.entry((*action).to_string()).or_insert(0) += 1;
                    match action.to_ascii_lowercase().as_str() {
                        "read" => reads += 1,
                        "edit" | "write" => edits += 1,
                        _ => {}
                    }
                }
            }
            let lower = line.to_ascii_lowercase();
            if TEST_MARKERS.iter().any(|marker| lower.contains(marker)) {
                tests += 1;
            }
            push_files(line, &mut files, &mut seen);
        }

        let approach = if tests > 0 && edits > 0 {
            "tdd"
        } else if reads > edits * 2 {
            "investigation"
        } else if edits > 0 {
            "direct_fix"
        } else if reads > 0 {
            "exploration"
        } else {
            "unknown"
        }
        .to_string();

        let summary = format!(
            "{} files explored, {} tests run, approach: {approach}",
            files.len(),
            tests
        );

        Self {
            approach,
            files,
            actions: counts,
            tests,
            summary,
        }
    }

    /// Prompt block listing up to three fingerprints. Empty when `items` is.
    pub fn as_prompt(items: &[Self]) -> String {
        if items.is_empty() {
            return String::new();
        }
        let mut lines = Vec::from(["# Successful strategies".to_string(), String::new()]);
        for (index, item) in items.iter().take(3).enumerate() {
            lines.push(format!(
                "{}. **{}**: {}",
                index + 1,
                item.approach,
                item.summary
            ));
            if !item.files.is_empty() {
                let preview: Vec<&str> = item.files.iter().take(5).map(String::as_str).collect();
                lines.push(format!("   Key files: {}", preview.join(", ")));
            }
        }
        lines.push(String::new());
        lines.join("\n")
    }
}

pub(crate) fn push_files(
    line: &str,
    files: &mut Vec<String>,
    seen: &mut std::collections::BTreeSet<String>,
) {
    if let Some(path) = diff_path(line) {
        if seen.insert(path.clone()) {
            files.push(path);
        }
        return;
    }
    for token in line.split_whitespace() {
        let token = token.trim_matches(|character: char| {
            character == ',' || character == ';' || character == ':' || character == '"'
        });
        if looks_like_file(token) && seen.insert(token.to_string()) {
            files.push(token.to_string());
        }
    }
}

pub(crate) fn diff_path(line: &str) -> Option<String> {
    let rest = line
        .strip_prefix("+++ b/")
        .or_else(|| line.strip_prefix("--- a/"))?;
    let path = rest.trim();
    if path.is_empty() {
        None
    } else {
        Some(path.to_string())
    }
}

pub(crate) fn looks_like_file(token: &str) -> bool {
    FILE_PREFIXES.iter().any(|prefix| token.starts_with(prefix)) && token.contains('.')
}

pub(crate) fn mentions_test(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    TEST_MARKERS.iter().any(|marker| lower.contains(marker))
        || lower.contains("test passed")
        || lower.contains("test failed")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_tdd_when_edits_and_tests_both_appear() {
        let fingerprint = Fingerprint::parse(
            "Read src/main.rs\nEdit src/handler.rs\nRunning cargo test\nEdit tests/test_handler.rs\n",
            &["Read", "Edit", "Write", "Bash", "Grep", "Glob"],
        );
        assert_eq!(fingerprint.approach, "tdd");
        assert!(fingerprint.tests >= 1);
        assert!(fingerprint.files.contains(&"src/main.rs".to_string()));
        assert_eq!(*fingerprint.actions.get("Read").unwrap_or(&0), 1);
        assert!(Fingerprint::as_prompt(&[fingerprint]).contains("tdd"));
    }

    #[test]
    fn parse_direct_fix_and_investigation() {
        let direct = Fingerprint::parse(
            "Read src/config.rs\nEdit src/config.rs\nWrite src/new_file.rs\n",
            &["Read", "Edit", "Write"],
        );
        assert_eq!(direct.approach, "direct_fix");
        assert_eq!(direct.tests, 0);

        let investigation = Fingerprint::parse(
            "Read src/main.rs\nRead src/handler.rs\nRead src/config.rs\nRead src/types.rs\nRead src/storage/mod.rs\nRead src/storage/sqlite.rs\nEdit src/handler.rs\n",
            &["Read", "Edit"],
        );
        assert_eq!(investigation.approach, "investigation");
    }

    #[test]
    fn empty_text_is_unknown_and_prompt_is_empty() {
        let fingerprint = Fingerprint::parse("", &["Read"]);
        assert_eq!(fingerprint.approach, "unknown");
        assert!(fingerprint.files.is_empty());
        assert!(Fingerprint::as_prompt(&[]).is_empty());
    }
}
