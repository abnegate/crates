//! Per-run execution logs.
//!
//! A run can keep three files under a root directory, rotated into one
//! directory per agent per UTC day: the agent's decoded prose, its raw stderr,
//! and a JSONL [`Journal`] of what happened to the process and every line it
//! printed. Logging is best effort throughout: a log that cannot be written is
//! reported through `tracing` and never fails the run it describes.
//!
//! Logs hold what the agent read and said, so each directory is created
//! readable by its owner alone, and each file is created afresh, readable by
//! its owner alone, and never through a link someone else planted.

mod files;
mod journal;
mod record;
mod sink;
mod writer;

use std::ffi::OsString;
use std::path::Path;
use std::path::PathBuf;

pub use crate::log::files::ExecutionLogFiles;
pub use crate::log::journal::Journal;
pub use crate::log::record::Record;
pub(crate) use crate::log::sink::Sink;

/// The directory logs go in, beneath the platform's per-user state
/// directory, when none is configured.
pub const LOG_DIRECTORY_NAME: &str = "abnegate-agent-cli";

/// How much of a run's output a summary keeps.
pub const EXECUTION_LOG_PREVIEW_LIMIT: usize = 2000;

const ELLIPSIS: &str = "...";

const PRIVATE: u32 = 0o600;
const PRIVATE_DIRECTORY: u32 = 0o700;

const HOME: &str = "HOME";
#[cfg(target_os = "macos")]
const STATE: [&str; 2] = ["Library", "Logs"];
#[cfg(not(target_os = "macos"))]
const STATE: [&str; 2] = [".local", "state"];
#[cfg(not(target_os = "macos"))]
const XDG_STATE_HOME: &str = "XDG_STATE_HOME";

/// The log root named by the environment variable `variable`, or
/// [`default_log_directory`] when it is unset. A variable set to the empty
/// string resolves to an empty path, which disables logging, as does having
/// no default to fall back on.
pub fn resolve_log_root(variable: &str) -> PathBuf {
    std::env::var_os(variable).map_or_else(
        || default_log_directory().unwrap_or_default(),
        PathBuf::from,
    )
}

/// [`LOG_DIRECTORY_NAME`] beneath the platform's per-user state directory:
/// `$XDG_STATE_HOME`, or `~/.local/state`, and `~/Library/Logs` on macOS.
/// `None` when there is no home directory to put it under.
pub fn default_log_directory() -> Option<PathBuf> {
    directory_from(&|variable| std::env::var_os(variable))
}

fn directory_from(variable: &dyn Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    #[cfg(not(target_os = "macos"))]
    if let Some(state) = variable(XDG_STATE_HOME)
        .map(PathBuf::from)
        .filter(|state| state.is_absolute())
    {
        return Some(state.join(LOG_DIRECTORY_NAME));
    }
    let home = variable(HOME)
        .map(PathBuf::from)
        .filter(|home| home.is_absolute())?;
    Some(
        STATE
            .iter()
            .fold(home, |path, component| path.join(component))
            .join(LOG_DIRECTORY_NAME),
    )
}

/// Create `directory` readable by its owner alone, or make an existing one
/// so, refusing one that is a link or cannot be made private. Its parent
/// must exist.
///
/// A link swapped in between the check and tightening an existing
/// directory's mode would have that mode applied through it; that needs
/// someone else able to write to the log root, which should be the owner's
/// alone.
fn private_directory(directory: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    use std::os::unix::fs::PermissionsExt;

    match std::fs::DirBuilder::new()
        .mode(PRIVATE_DIRECTORY)
        .create(directory)
    {
        Err(error) if error.kind() != std::io::ErrorKind::AlreadyExists => return Err(error),
        _ => {}
    }
    let metadata = std::fs::symlink_metadata(directory)?;
    if !metadata.file_type().is_dir() {
        return Err(std::io::Error::other(format!(
            "{} is not a directory",
            directory.display()
        )));
    }
    if metadata.permissions().mode() & 0o777 != PRIVATE_DIRECTORY {
        std::fs::set_permissions(
            directory,
            std::fs::Permissions::from_mode(PRIVATE_DIRECTORY),
        )?;
    }
    Ok(())
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
    use std::collections::BTreeMap;
    use std::ffi::OsString;
    use std::path::PathBuf;

    use super::EXECUTION_LOG_PREVIEW_LIMIT;
    use super::default_log_directory;
    use super::directory_from;
    use super::preview;
    use super::resolve_log_root;

    fn environment(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
        let variables: BTreeMap<String, OsString> = pairs
            .iter()
            .map(|(name, value)| ((*name).to_string(), OsString::from(value)))
            .collect();
        move |name| variables.get(name).cloned()
    }

    #[test]
    fn an_unset_variable_resolves_to_the_default_directory() {
        let root = resolve_log_root("ABNEGATE_AGENT_CLI_UNSET_LOG_DIRECTORY_FOR_TESTS");
        assert_eq!(root, default_log_directory().unwrap_or_default());
        assert!(
            root.as_os_str().is_empty() || root.is_absolute(),
            "{}",
            root.display()
        );
        assert_ne!(root, PathBuf::from("./logs"));
    }

    #[test]
    fn the_default_directory_sits_beneath_the_platforms_state_directory() {
        let home = directory_from(&environment(&[("HOME", "/home/agent")]));

        #[cfg(target_os = "macos")]
        assert_eq!(
            home,
            Some(PathBuf::from("/home/agent/Library/Logs/abnegate-agent-cli"))
        );
        #[cfg(not(target_os = "macos"))]
        {
            assert_eq!(
                home,
                Some(PathBuf::from("/home/agent/.local/state/abnegate-agent-cli"))
            );
            assert_eq!(
                directory_from(&environment(&[
                    ("HOME", "/home/agent"),
                    ("XDG_STATE_HOME", "/state")
                ])),
                Some(PathBuf::from("/state/abnegate-agent-cli"))
            );
            assert_eq!(
                directory_from(&environment(&[
                    ("HOME", "/home/agent"),
                    ("XDG_STATE_HOME", "relative")
                ])),
                Some(PathBuf::from("/home/agent/.local/state/abnegate-agent-cli"))
            );
        }
        assert_eq!(directory_from(&environment(&[])), None);
        assert_eq!(directory_from(&environment(&[("HOME", "relative")])), None);
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
