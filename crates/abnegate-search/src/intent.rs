//! Deciding whether a message is worth a web lookup at all.

use crate::query::sanitize_query;

/// Shorter than this, a message is a reply rather than a question.
const MINIMUM_QUERY_LENGTH: usize = 8;

/// Heuristic: does this message benefit from live web results?
///
/// Keeps search off for code review, casual replies, and stable knowledge
/// questions; turns it on for news, prices, weather, recency, and lookups.
pub fn needs_web_search(content: &str) -> bool {
    let query = sanitize_query(content);
    if query.len() < MINIMUM_QUERY_LENGTH {
        return false;
    }

    let lower = query.to_lowercase();

    if looks_like_code_request(&lower, &query) {
        return false;
    }
    if is_casual_reply(&lower) {
        return false;
    }
    if contains_url(&query) {
        return true;
    }

    has_web_intent(&lower)
}

fn contains_url(text: &str) -> bool {
    text.contains("http://") || text.contains("https://") || text.contains("www.")
}

fn is_casual_reply(lower: &str) -> bool {
    const REPLIES: &[&str] = &[
        "thanks",
        "thank you",
        "ok",
        "okay",
        "yes",
        "no",
        "hello",
        "hi",
        "hey",
        "cool",
        "great",
        "got it",
        "sounds good",
        "perfect",
        "nice",
        "yep",
        "nope",
    ];
    REPLIES.contains(&lower.trim())
}

fn looks_like_code_request(lower: &str, raw: &str) -> bool {
    if raw.contains("```") {
        return true;
    }

    const CODE_PHRASES: &[&str] = &[
        "refactor",
        "stack trace",
        "compile error",
        "unit test",
        "explain this code",
        "review this pr",
        "review this pull request",
        "what does this function",
        "fix this bug",
        "fix this error",
        "lint error",
        "type error",
        "syntax error",
    ];
    if CODE_PHRASES.iter().any(|phrase| lower.contains(phrase)) {
        return true;
    }

    const CODE_MARKERS: &[&str] = &[
        "fn ",
        "def ",
        "class ",
        "import ",
        "export ",
        "const ",
        "let ",
        "struct ",
        "impl ",
        "async fn",
        "public void",
        "#include",
    ];
    CODE_MARKERS
        .iter()
        .filter(|marker| lower.contains(*marker))
        .count()
        >= 2
}

fn has_web_intent(lower: &str) -> bool {
    const STRONG: &[&str] = &[
        "latest",
        "recent",
        "currently",
        "today",
        "tonight",
        "yesterday",
        "this week",
        "this month",
        "right now",
        "as of",
        "news",
        "headline",
        "breaking",
        "weather",
        "forecast",
        "stock price",
        "share price",
        "market cap",
        "who won",
        "who is the current",
        "who is the new",
        "election result",
        "release date",
        "when will it release",
        "when will it launch",
        "search for",
        "look up",
        "find online",
        "on the internet",
        "on the web",
        "web search",
        "price of",
        "cost of",
        "live score",
        "exchange rate",
    ];
    if STRONG.iter().any(|phrase| lower.contains(phrase)) {
        return true;
    }

    const RECENT_YEARS: &[&str] = &["2024", "2025", "2026", "2027"];
    if lower.contains('?') && RECENT_YEARS.iter().any(|year| lower.contains(year)) {
        return true;
    }

    const FRESH_QUESTIONS: &[&str] = &[
        "what happened",
        "what's new",
        "whats new",
        "what is new",
        "what are the latest",
        "what is the latest",
        "who is running",
        "who is president",
        "who is ceo",
        "when did ",
        "where can i buy",
        "is it out yet",
        "is it available",
    ];
    FRESH_QUESTIONS.iter().any(|phrase| lower.contains(phrase))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn needs_web_search_detects_recency_and_skips_code() {
        assert!(!needs_web_search("thanks"));
        assert!(!needs_web_search("Explain this function"));
        assert!(!needs_web_search("Refactor this Rust module"));
        assert!(!needs_web_search("What is a binary search tree?"));
        assert!(!needs_web_search(
            "What is the current implementation of this parser?"
        ));
        assert!(needs_web_search("What is the latest news on OpenAI?"));
        assert!(needs_web_search("What's the weather in Auckland today?"));
        assert!(needs_web_search("Who won the game last night?"));
        assert!(needs_web_search("Check https://example.com and summarize"));
        assert!(needs_web_search("What happened in 2026?"));
    }
}
