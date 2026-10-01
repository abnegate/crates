//! Keyword tags extracted from trial text.

const COMMON: &[&str] = &[
    "the",
    "a",
    "an",
    "is",
    "are",
    "was",
    "were",
    "be",
    "been",
    "being",
    "have",
    "has",
    "had",
    "do",
    "does",
    "did",
    "will",
    "would",
    "could",
    "should",
    "may",
    "might",
    "must",
    "shall",
    "can",
    "need",
    "dare",
    "this",
    "that",
    "these",
    "those",
    "what",
    "which",
    "who",
    "whom",
    "when",
    "where",
    "why",
    "how",
    "all",
    "each",
    "every",
    "both",
    "few",
    "more",
    "most",
    "other",
    "some",
    "such",
    "than",
    "too",
    "very",
    "just",
    "also",
    "only",
    "now",
    "then",
    "here",
    "there",
    "with",
    "from",
    "into",
    "onto",
    "upon",
    "over",
    "under",
    "above",
    "below",
    "between",
    "among",
    "through",
    "during",
    "before",
    "after",
    "about",
    "against",
    "without",
    "within",
    "throughout",
    "around",
    "and",
    "but",
    "or",
    "nor",
    "for",
    "yet",
    "so",
    "because",
    "although",
    "while",
    "if",
    "unless",
    "until",
    "since",
    "once",
    "whereas",
    "error",
    "issue",
    "problem",
    "bug",
    "fix",
    "fixed",
    "fixing",
];

/// Whether `word` is too common to keep as a tag.
pub fn is_common(word: &str) -> bool {
    COMMON.contains(&word)
}

/// Distinct significant words from `text`, longest-kept first, at most twenty.
///
/// Words of three characters or fewer, and [`is_common`] words, are dropped.
pub fn tags_from(text: &str) -> Vec<String> {
    let lower = text.to_ascii_lowercase();
    let mut tags = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for word in lower.split(|character: char| !character.is_alphanumeric() && character != '_') {
        if word.len() <= 3 || is_common(word) || !seen.insert(word.to_string()) {
            continue;
        }
        tags.push(word.to_string());
        if tags.len() == 20 {
            break;
        }
    }
    tags
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_drop_short_and_common_words() {
        let tags = tags_from(
            "Database connection timeout. The database connection times out when processing large queries",
        );
        assert!(tags.contains(&"database".to_string()));
        assert!(tags.contains(&"connection".to_string()));
        assert!(tags.contains(&"timeout".to_string()));
        assert!(tags.contains(&"queries".to_string()));
        assert!(!tags.iter().any(|tag| tag.len() <= 3));
        assert!(!tags.iter().any(|tag| is_common(tag)));
    }

    #[test]
    fn tags_cap_at_twenty_and_empty_text_is_empty() {
        let long = (1..=25)
            .map(|index| format!("word{index}"))
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(tags_from(&long).len(), 20);
        assert!(tags_from("").is_empty());
        assert!(is_common("the"));
        assert!(!is_common("database"));
    }
}
