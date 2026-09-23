//! Length limits shared by the backends.

const ELLIPSIS: char = '\u{2026}';

/// `text` shortened to `limit` characters, ending in an ellipsis when cut.
///
/// Every provider rejects an over-long field outright, so a message is
/// truncated rather than lost. Counting characters and cutting on a boundary
/// keeps multi-byte text from being split mid-codepoint.
pub(crate) fn truncate(text: &str, limit: usize) -> String {
    debug_assert!(
        limit > 0,
        "a zero limit would leave nothing but an ellipsis"
    );
    match text.char_indices().nth(limit) {
        None => text.to_string(),
        Some(_) => {
            let keep = limit.saturating_sub(1);
            let end = text
                .char_indices()
                .nth(keep)
                .map(|(index, _)| index)
                .unwrap_or(text.len());
            let mut truncated = String::with_capacity(end + ELLIPSIS.len_utf8());
            truncated.push_str(&text[..end]);
            truncated.push(ELLIPSIS);
            truncated
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_within_the_limit_is_untouched() {
        assert_eq!(truncate("hello", 5), "hello");
        assert_eq!(truncate("hello", 50), "hello");
        assert_eq!(truncate("", 10), "");
    }

    #[test]
    fn longer_text_is_cut_to_the_limit() {
        let truncated = truncate("hello world", 8);
        assert_eq!(truncated, "hello w\u{2026}");
        assert_eq!(truncated.chars().count(), 8);
    }

    #[test]
    fn multibyte_text_is_cut_on_a_character_boundary() {
        let truncated = truncate("héllo wörld 中文", 8);
        assert_eq!(truncated.chars().count(), 8);
        assert!(std::str::from_utf8(truncated.as_bytes()).is_ok());
        assert!(truncated.ends_with(ELLIPSIS));
    }

    #[test]
    fn a_single_character_over_the_limit_still_truncates() {
        assert_eq!(truncate("abcdef", 5), "abcd\u{2026}");
    }
}
