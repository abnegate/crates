//! Per-run execution logs.
//!
//! A run can keep three files under a root directory, rotated into one
//! directory per agent per UTC day: the agent's decoded prose, its raw stderr,
//! and a JSONL [`Journal`] of what happened to the process and every line it
//! printed. Logging is best effort throughout: a log that cannot be written is
//! reported through `tracing` and never fails the run it describes.

mod files;
mod journal;
mod record;
mod sink;

use std::path::PathBuf;

pub use crate::log::files::ExecutionLogFiles;
pub use crate::log::journal::Journal;
pub use crate::log::record::Record;
pub(crate) use crate::log::sink::Sink;

/// Where logs go when the configured variable is unset.
pub const DEFAULT_LOG_DIRECTORY: &str = "./logs";

/// How much of a run's output a summary keeps.
pub const EXECUTION_LOG_PREVIEW_LIMIT: usize = 2000;

const ELLIPSIS: &str = "...";

/// Logs hold what the agent read and said, so only their owner may read them.
const PRIVATE: u32 = 0o600;

/// The log root named by the environment variable `variable`, or
/// [`DEFAULT_LOG_DIRECTORY`] when it is unset. A variable set to the empty
/// string resolves to an empty path, which disables logging.
pub fn resolve_log_root(variable: &str) -> PathBuf {
    std::env::var_os(variable).map_or_else(|| PathBuf::from(DEFAULT_LOG_DIRECTORY), PathBuf::from)
}

/// `text` cut to at most `limit` bytes on a character boundary, ending in an
/// ellipsis when anything was cut.
pub fn preview(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_string();
    }
    let mut end = limit.saturating_sub(ELLIPSIS.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{ELLIPSIS}", &text[..end])
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::DEFAULT_LOG_DIRECTORY;
    use super::EXECUTION_LOG_PREVIEW_LIMIT;
    use super::preview;
    use super::resolve_log_root;

    #[test]
    fn an_unset_variable_resolves_to_the_default_directory() {
        let root = resolve_log_root("ABNEGATE_AGENT_CLI_UNSET_LOG_DIRECTORY_FOR_TESTS");
        assert_eq!(root, PathBuf::from(DEFAULT_LOG_DIRECTORY));
        assert_eq!(DEFAULT_LOG_DIRECTORY, "./logs");
    }

    #[test]
    fn a_set_variable_names_the_root() {
        let Some(home) = std::env::var_os("HOME") else {
            return;
        };
        assert_eq!(resolve_log_root("HOME"), PathBuf::from(home));
    }

    #[test]
    fn the_preview_limit_is_two_thousand_bytes() {
        assert_eq!(EXECUTION_LOG_PREVIEW_LIMIT, 2000);
    }

    #[test]
    fn text_within_the_limit_is_untouched() {
        assert_eq!(preview("hello", 100), "hello");
        assert_eq!(preview("hello", 5), "hello");
        assert_eq!(preview("abc", 3), "abc");
        assert_eq!(preview("0123456789", 10), "0123456789");
        assert_eq!(preview("short", 1000), "short");
        assert_eq!(preview("", 0), "");
    }

    #[test]
    fn text_past_the_limit_ends_in_an_ellipsis_within_the_limit() {
        assert_eq!(preview("abcdef", 5), "ab...");
        assert_eq!(preview("abcdef", 4), "a...");
        assert_eq!(preview("01234567890", 10), "0123456...");

        let long = preview(&"a".repeat(10_000), 100);
        assert!(long.ends_with("..."));
        assert_eq!(long.len(), 100);
    }

    #[test]
    fn a_limit_too_small_for_any_text_leaves_only_the_ellipsis() {
        for limit in [0, 1, 2, 3] {
            assert_eq!(preview("abcdef", limit), "...", "limit {limit}");
        }
        assert_eq!(preview("abcd", 3), "...");
        assert_eq!(preview("hello", 0), "...");
    }

    #[test]
    fn a_multibyte_character_is_never_split() {
        assert_eq!(preview("aéb", 3), "...");

        for (text, limit) in [
            ("\u{1F600}abc", 5),
            ("\u{00e9}\u{00e9}\u{00e9}\u{00e9}", 6),
            ("\u{4e16}\u{754c}hello", 8),
            ("e\u{0301}abc", 5),
        ] {
            let cut = preview(text, limit);
            assert!(cut.ends_with("..."), "{text:?} at {limit}: {cut:?}");
            assert!(cut.len() <= limit.max(3), "{text:?} at {limit}: {cut:?}");
            assert!(std::str::from_utf8(cut.as_bytes()).is_ok());
        }
    }
}
