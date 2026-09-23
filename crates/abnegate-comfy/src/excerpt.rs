//! Bounded, sanitized quotes of text that came from another process or server.

use abnegate_secret::sanitize;

/// Most characters of outside text that reach one log line or error message.
pub(crate) const LIMIT: usize = 512;
const ELISION: char = '…';

/// The end of `text`, stripped of terminal control sequences and credentials.
/// A failing process says why last.
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
        assert_eq!(tail("  exit 3\n"), "exit 3");
    }

    #[test]
    fn long_text_is_cut_to_its_last_characters() {
        let text = format!("{}{}", "a".repeat(LIMIT), "b".repeat(LIMIT));
        let end = tail(&text);
        assert_eq!(end.chars().count(), LIMIT + 1);
        assert!(end.starts_with(ELISION) && end.ends_with('b'));
        assert!(!end.contains('a'));
    }

    #[test]
    fn a_cut_never_splits_a_character() {
        assert_eq!(tail(&"é".repeat(LIMIT * 2)).chars().count(), LIMIT + 1);
    }

    #[test]
    fn control_sequences_and_credentials_never_reach_the_quote() {
        let quote = tail(concat!(
            "\u{1b}[31mfatal\u{1b}[0m: bad token ghp_",
            "0123456789abcdefghij0123456789abcdef"
        ));
        assert!(!quote.contains('\u{1b}'), "{quote:?}");
        assert!(!quote.contains(concat!("ghp_", "0123456789")), "{quote:?}");
        assert!(quote.starts_with("fatal"), "{quote:?}");
    }
}
