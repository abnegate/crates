//! How hard a thinking-capable model should think for this request.

mod effort;
mod reasoning_effort;

pub use crate::reasoning::effort::Effort;
pub use crate::reasoning::reasoning_effort::ReasoningEffort;

const GREETINGS: &[&str] = &[
    "hi",
    "hello",
    "hey",
    "thanks",
    "thank you",
    "ok",
    "okay",
    "yes",
    "no",
    "yep",
    "nope",
    "sure",
    "cool",
    "great",
    "got it",
    "please",
    "good morning",
    "good night",
    "gm",
    "ty",
];

const HIGH_MARKERS: &[&str] = &[
    "implement",
    "refactor",
    "architecture",
    "architect",
    "debug",
    "diagnose",
    "deadlock",
    "race condition",
    "prove",
    "theorem",
    "algorithm",
    "trade-off",
    "tradeoff",
    "root cause",
    "step by step",
    "systematically",
    "thoroughly",
    "in detail",
    "why does",
    "why is",
    "how should",
];

const LONG_PROMPT: usize = 1200;
const MANY_QUESTIONS: usize = 3;
const MANY_LINES: usize = 7;
const MANY_STEPS: usize = 3;

/// How hard a request reads as being.
pub fn classify(prompt: &str) -> Effort {
    let text = prompt.trim();
    if text.is_empty() {
        return Effort::Low;
    }
    let lower = text.to_ascii_lowercase();
    if is_high(&lower, text) {
        Effort::High
    } else if is_low(&lower) {
        Effort::Low
    } else {
        Effort::Medium
    }
}

fn is_low(lower: &str) -> bool {
    let stripped = lower
        .trim_matches(|character: char| matches!(character, '!' | '.' | '?' | ',' | ';' | ' '));
    GREETINGS.contains(&stripped)
}

fn is_high(lower: &str, text: &str) -> bool {
    if text.len() > LONG_PROMPT || lower.contains("```") {
        return true;
    }
    if lower.chars().filter(|character| *character == '?').count() >= MANY_QUESTIONS {
        return true;
    }
    if lower.matches('\n').count() >= MANY_LINES {
        return true;
    }
    if (1..=6)
        .filter(|index| {
            lower.contains(&format!("\n{index}. ")) || lower.starts_with(&format!("{index}. "))
        })
        .count()
        >= MANY_STEPS
    {
        return true;
    }
    HIGH_MARKERS.iter().any(|marker| lower.contains(marker))
}

#[cfg(test)]
mod tests {
    use super::{Effort, ReasoningEffort, classify};

    #[test]
    fn auto_picks_effort_from_the_request() {
        assert_eq!(classify("Thanks!"), Effort::Low);
        assert_eq!(classify("What is the capital of France?"), Effort::Medium);
        assert_eq!(
            classify("Debug the deadlock in the worker pool step by step."),
            Effort::High
        );
        assert_eq!(ReasoningEffort::Auto.resolve("Hello."), Some(Effort::Low));
        assert_eq!(ReasoningEffort::Off.resolve("Debug this."), None);
        assert_eq!(ReasoningEffort::High.resolve("Hi"), Some(Effort::High));
    }

    #[test]
    fn long_or_fenced_requests_are_high() {
        assert_eq!(classify(&"paragraph\n".repeat(12)), Effort::High);
        assert_eq!(
            classify("Use this:\n```rust\nfn main() {}\n```"),
            Effort::High
        );
        assert_eq!(
            classify("1. Collect facts\n2. Compare designs\n3. Recommend one"),
            Effort::High
        );
    }

    #[test]
    fn an_empty_request_needs_no_thinking() {
        assert_eq!(classify("   "), Effort::Low);
    }
}
