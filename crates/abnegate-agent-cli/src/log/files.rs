use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

const LABEL_LIMIT: usize = 64;
const UNLABELLED: &str = "run";
const DAY: &str = "%Y-%m-%d";
const TIMESTAMP: &str = "%Y%m%dT%H%M%S%.3fZ";

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Where one run's logs are written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionLogFiles {
    /// The agent's decoded prose, scrubbed of secrets.
    pub stdout: PathBuf,
    /// The agent's stderr, a line at a time and scrubbed of secrets. A line
    /// past the line limit is left out; the run's own copy of its
    /// diagnostics keeps it.
    pub stderr: PathBuf,
    /// The run's [`Journal`](crate::log::Journal), one JSON object per line.
    pub events: PathBuf,
}

impl ExecutionLogFiles {
    /// Name one run's files under `root/<agent>/<UTC day>/`, creating that
    /// directory.
    ///
    /// The three names share a stem of timestamp, process, sequence and
    /// `label`, so concurrent runs never share a file and a run's files sort
    /// together. `None` when `root` is empty, which disables logging, or when
    /// the directory cannot be created.
    pub fn create(root: &Path, agent: &str, label: &str) -> Option<Self> {
        if root.as_os_str().is_empty() {
            return None;
        }

        let now = chrono::Utc::now();
        let directory = root.join(sanitize(agent)).join(now.format(DAY).to_string());

        if let Err(error) = std::fs::create_dir_all(&directory) {
            tracing::warn!(
                path = %directory.display(),
                %error,
                "could not create the execution log directory"
            );
            return None;
        }

        let stem = format!(
            "{}_{}-{}_{}",
            now.format(TIMESTAMP),
            std::process::id(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed),
            sanitize(label)
        );

        Some(Self {
            stdout: directory.join(format!("{stem}.stdout.log")),
            stderr: directory.join(format!("{stem}.stderr.log")),
            events: directory.join(format!("{stem}.events.jsonl")),
        })
    }
}

fn sanitize(label: &str) -> String {
    let sanitized: String = label
        .chars()
        .take(LABEL_LIMIT)
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' || character == '_' {
                character
            } else {
                '_'
            }
        })
        .collect();
    if sanitized.is_empty() {
        UNLABELLED.to_string()
    } else {
        sanitized
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::path::PathBuf;

    use tempfile::TempDir;

    use super::ExecutionLogFiles;
    use super::sanitize;

    fn file_name(path: &Path) -> &str {
        path.file_name()
            .and_then(|name| name.to_str())
            .expect("a file name")
    }

    #[test]
    fn a_label_keeps_only_safe_characters() {
        for (label, expected) in [
            ("hello123", "hello123"),
            ("hello world.foo/bar", "hello_world_foo_bar"),
            ("café☕日本", "caf____"),
            ("my-label_name", "my-label_name"),
            ("@#$", "___"),
            ("12345", "12345"),
            ("a", "a"),
            ("@", "_"),
            ("PROJ-123/fix", "PROJ-123_fix"),
            ("a\tb\nc", "a_b_c"),
            ("123abc", "123abc"),
            ("---___", "---___"),
            ("v1.2.3", "v1_2_3"),
            ("scope:task", "scope_task"),
            ("fix (urgent)", "fix__urgent_"),
        ] {
            assert_eq!(sanitize(label), expected, "{label:?}");
        }
    }

    #[test]
    fn an_empty_label_gets_a_placeholder() {
        assert_eq!(sanitize(""), "run");
    }

    #[test]
    fn a_label_is_capped_at_sixty_four_characters() {
        assert_eq!(sanitize(&"a".repeat(100)).len(), 64);
        assert_eq!(sanitize(&"a".repeat(65)).len(), 64);
        assert_eq!(sanitize(&"a".repeat(64)).len(), 64);
        assert_eq!(sanitize(&"\u{1F600}".repeat(100)), "_".repeat(64));
    }

    #[test]
    fn an_all_non_ascii_label_is_all_underscores_not_the_placeholder() {
        let sanitized = sanitize("\u{1F600}\u{1F601}");
        assert_eq!(sanitized, "__");
    }

    #[test]
    fn three_files_share_one_dated_directory_and_stem() {
        let root = TempDir::new().expect("a temporary directory");
        let files =
            ExecutionLogFiles::create(root.path(), "claude", "test-label").expect("log files");

        assert_eq!(files.stdout.parent(), files.stderr.parent());
        assert_eq!(files.stderr.parent(), files.events.parent());

        let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
        let directory = files.stdout.parent().expect("a directory");
        assert_eq!(directory, root.path().join("claude").join(&today));
        assert!(directory.is_dir());

        let stdout = file_name(&files.stdout);
        let stderr = file_name(&files.stderr);
        let events = file_name(&files.events);
        assert!(stdout.ends_with("_test-label.stdout.log"), "{stdout}");
        assert!(stderr.ends_with(".stderr.log"), "{stderr}");
        assert!(events.ends_with(".events.jsonl"), "{events}");

        let stem = stdout.trim_end_matches(".stdout.log");
        assert!(stderr.starts_with(stem));
        assert!(events.starts_with(stem));
        assert!(
            stem.contains(&format!("_{}-", std::process::id())),
            "{stem} lacks the process id"
        );
    }

    #[test]
    fn an_unsafe_label_never_reaches_a_file_name() {
        let root = TempDir::new().expect("a temporary directory");
        let files = ExecutionLogFiles::create(root.path(), "claude", "hello world/foo@bar")
            .expect("log files");

        let stdout = file_name(&files.stdout);
        assert!(!stdout.contains(' '));
        assert!(!stdout.contains('@'));
        assert_eq!(
            files.stdout.parent().and_then(Path::parent),
            Some(root.path().join("claude").as_path())
        );
    }

    #[test]
    fn two_runs_with_one_label_never_share_a_file() {
        let root = TempDir::new().expect("a temporary directory");
        let first = ExecutionLogFiles::create(root.path(), "codex", "same").expect("log files");
        let second = ExecutionLogFiles::create(root.path(), "codex", "same").expect("log files");

        assert_ne!(first.stdout, second.stdout);
        assert_ne!(first.events, second.events);
    }

    #[test]
    fn an_empty_root_disables_logging() {
        assert!(ExecutionLogFiles::create(Path::new(""), "claude", "test").is_none());
    }

    #[test]
    fn a_root_that_cannot_be_created_disables_logging() {
        let root = TempDir::new().expect("a temporary directory");
        let blocker = root.path().join("file");
        std::fs::write(&blocker, b"not a directory").expect("a file");

        assert!(ExecutionLogFiles::create(&blocker, "claude", "test").is_none());
    }

    #[test]
    fn files_clone_and_debug_by_field() {
        let files = ExecutionLogFiles {
            stdout: PathBuf::from("/tmp/test.stdout.log"),
            stderr: PathBuf::from("/tmp/test.stderr.log"),
            events: PathBuf::from("/tmp/test.events.jsonl"),
        };

        assert_eq!(files.clone(), files);
        let debug = format!("{files:?}");
        assert!(debug.contains("stdout"));
        assert!(debug.contains("stderr"));
        assert!(debug.contains("events"));
    }
}
