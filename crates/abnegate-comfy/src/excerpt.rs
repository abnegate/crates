//! Bounded, sanitized quotes of text that came from another process or server.

use abnegate_secret::sanitize;

/// Most characters of outside text that reach one log line or error message.
pub(crate) const LIMIT: usize = 512;
const ELISION: char = '…';

/// The start of `text`, stripped of terminal control sequences and credentials.
pub(crate) fn head(text: &str) -> String {
    let clean = sanitize(text.trim());
    match clean.char_indices().nth(LIMIT) {
        Some((cut, _)) => format!("{}{ELISION}", &clean[..cut]),
        None => clean.into_owned(),
    }
}

/// The end of `text`, stripped the same way. A failing process says why last.
pub(crate) fn tail(text: &str) -> String {
    let clean = sanitize(text.trim());
    let length = clean.chars().count();
    match clean.char_indices().nth(length.saturating_sub(LIMIT)) {
        Some((cut, _)) if cut > 0 => format!("{ELISION}{}", &clean[cut..]),
        _ => clean.into_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_text_is_quoted_whole() {
        assert_eq!(head("  exit 3\n"), "exit 3");
        assert_eq!(tail("  exit 3\n"), "exit 3");
    }

    #[test]
    fn long_text_is_cut_to_the_limit_from_the_end_that_matters() {
        let text = format!("{}{}", "a".repeat(LIMIT), "b".repeat(LIMIT));
        let start = head(&text);
        assert_eq!(start.chars().count(), LIMIT + 1);
        assert!(start.starts_with('a') && start.ends_with(ELISION));
        let end = tail(&text);
        assert_eq!(end.chars().count(), LIMIT + 1);
        assert!(end.starts_with(ELISION) && end.ends_with('b'));
        assert!(!end.contains('a'));
    }

    #[test]
    fn a_cut_never_splits_a_character() {
        let text = "é".repeat(LIMIT * 2);
        assert_eq!(head(&text).chars().count(), LIMIT + 1);
        assert_eq!(tail(&text).chars().count(), LIMIT + 1);
    }

    #[test]
    fn control_sequences_and_credentials_never_reach_the_quote() {
        let text = concat!(
            "\u{1b}[31mfatal\u{1b}[0m: bad token ghp_",
            "0123456789abcdefghij0123456789abcdef"
        );
        for quote in [head(text), tail(text)] {
            assert!(!quote.contains('\u{1b}'), "{quote:?}");
            assert!(!quote.contains(concat!("ghp_", "0123456789")), "{quote:?}");
            assert!(quote.starts_with("fatal"), "{quote:?}");
        }
    }
}
