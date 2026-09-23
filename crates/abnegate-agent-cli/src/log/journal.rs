use std::fmt;
use std::path::Path;
use std::sync::Arc;

use serde_json::json;
use tokio::fs::OpenOptions;
use tokio::sync::Mutex;

use crate::log::PRIVATE;
use crate::log::record::Record;
use crate::log::writer::Writer;

/// An append-only JSONL account of one run, shared by the tasks that drive it.
///
/// A disabled journal accepts every entry and writes none, so a caller never
/// branches on whether logging is on. The provider's own entries are named by
/// [`Record`](crate::log::Record); a caller can add its own under names of its
/// choosing to the same file.
///
/// Entries for the lines a run printed are buffered and stop for good at
/// the first that would take the file past its limit, with one
/// [`Record::Truncated`] entry saying so. Every other entry is always written
/// and flushes whatever is buffered, so it reaches the disk with every line
/// before it; only lines since the last such entry are lost if the process
/// itself dies.
#[derive(Debug, Clone, Default)]
pub struct Journal {
    label: String,
    writer: Option<Arc<Mutex<Writer>>>,
}

impl Journal {
    pub fn disabled() -> Self {
        Self::default()
    }

    /// Start the journal at `path`, holding the lines a run printed to
    /// `limit` bytes of it, or a disabled journal when it cannot be created.
    ///
    /// The file must not exist yet: creating it exclusively also refuses a
    /// link planted in its place, which `O_CREAT | O_EXCL` never follows.
    pub async fn open(path: &Path, label: impl Into<String>, limit: u64) -> Self {
        let label = label.into();
        match OpenOptions::new()
            .create_new(true)
            .append(true)
            .mode(PRIVATE)
            .open(path)
            .await
        {
            Ok(file) => Self {
                label,
                writer: Some(Arc::new(Mutex::new(Writer::new(file, limit)))),
            },
            Err(error) => {
                tracing::warn!(
                    path = %path.display(),
                    %error,
                    "could not open the execution journal"
                );
                Self::disabled()
            }
        }
    }

    pub fn enabled(&self) -> bool {
        self.writer.is_some()
    }

    /// Append one entry and flush it, with everything buffered before it.
    /// Each entry is a single write of a whole line, so concurrent writers
    /// never interleave within one.
    pub async fn append(&self, event: impl fmt::Display, data: serde_json::Value) {
        self.write(event, data, false).await;
    }

    /// Append the entry for one line a run printed, buffered and dropped
    /// once the journal is full.
    pub(crate) async fn append_line(&self, record: Record, data: serde_json::Value) {
        self.write(record, data, true).await;
    }

    async fn write(&self, event: impl fmt::Display, data: serde_json::Value, line: bool) {
        let Some(writer) = &self.writer else {
            return;
        };
        let Some(entry) = self.entry(&event, data) else {
            return;
        };

        let mut writer = writer.lock().await;
        let written = if !line {
            match writer.write(&entry).await {
                Ok(()) => writer.flush().await,
                Err(error) => Err(error),
            }
        } else if writer.truncated() {
            Ok(())
        } else if !writer.full(entry.len()) {
            writer.write(&entry).await
        } else {
            writer.truncate();
            let limit = writer.limit();
            match self.entry(&Record::Truncated, json!({ "limit": limit })) {
                Some(marker) => writer.write(&marker).await,
                None => Ok(()),
            }
        };
        if let Err(error) = written {
            tracing::warn!(label = %self.label, %event, %error, "could not write a journal entry");
        }
    }

    fn entry(&self, event: &impl fmt::Display, data: serde_json::Value) -> Option<Vec<u8>> {
        let entry = json!({
            "timestamp": chrono::Utc::now().to_rfc3339(),
            "label": self.label,
            "event": event.to_string(),
            "data": data,
        });
        match serde_json::to_vec(&entry) {
            Ok(mut line) => {
                line.push(b'\n');
                Some(line)
            }
            Err(error) => {
                tracing::warn!(label = %self.label, %event, %error, "could not serialise a journal entry");
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::Value;
    use serde_json::json;
    use tempfile::TempDir;

    use super::Journal;
    use crate::log::record::Record;

    const LIMIT: u64 = 1024 * 1024;

    fn entries(path: &std::path::Path) -> Vec<Value> {
        std::fs::read_to_string(path)
            .expect("the journal")
            .lines()
            .map(|line| serde_json::from_str(line).expect("a JSON line"))
            .collect()
    }

    #[tokio::test]
    async fn a_disabled_journal_accepts_entries_and_writes_nothing() {
        let journal = Journal::disabled();
        assert!(!journal.enabled());
        journal
            .append(Record::Initialized, json!({"key": "value"}))
            .await;
    }

    #[tokio::test]
    async fn an_entry_is_one_line_naming_its_label_record_and_time() {
        let directory = TempDir::new().expect("a temporary directory");
        let path = directory.path().join("test_events.jsonl");
        let journal = Journal::open(&path, "my-label", LIMIT).await;
        assert!(journal.enabled());

        journal
            .append(Record::StdoutLine, json!({"foo": "bar", "count": 42}))
            .await;

        let [entry] = entries(&path).try_into().expect("one entry");
        assert_eq!(entry["label"], "my-label");
        assert_eq!(entry["event"], "stdout_line");
        assert_eq!(entry["data"]["foo"], "bar");
        assert_eq!(entry["data"]["count"], 42);
        assert!(
            chrono::DateTime::parse_from_rfc3339(entry["timestamp"].as_str().expect("a time"))
                .is_ok()
        );
    }

    #[tokio::test]
    async fn only_the_owner_can_read_a_journal() {
        use std::os::unix::fs::PermissionsExt;

        let directory = TempDir::new().expect("a temporary directory");
        let path = directory.path().join("events.jsonl");
        Journal::open(&path, "private", LIMIT)
            .await
            .append(Record::Initialized, json!({}))
            .await;

        let mode = std::fs::metadata(&path)
            .expect("metadata")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[tokio::test]
    async fn entries_are_appended_in_order() {
        let directory = TempDir::new().expect("a temporary directory");
        let path = directory.path().join("multi_events.jsonl");
        let journal = Journal::open(&path, "lbl", LIMIT).await;

        journal.append(Record::Spawned, json!({"seq": 1})).await;
        journal
            .clone()
            .append(Record::Exited, json!({"seq": 2}))
            .await;

        let entries = entries(&path);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0]["event"], "subprocess_spawned");
        assert_eq!(entries[1]["event"], "subprocess_exited");
        assert_eq!(entries[1]["data"]["seq"], 2);
    }

    #[tokio::test]
    async fn nested_data_survives() {
        let directory = TempDir::new().expect("a temporary directory");
        let path = directory.path().join("nested_events.jsonl");
        let journal = Journal::open(&path, "nested", LIMIT).await;

        journal
            .append(
                Record::Completed,
                json!({
                    "nested": {"key": "value", "arr": [1, 2, 3]},
                    "number": 42.5,
                    "bool": true,
                    "null_val": null
                }),
            )
            .await;

        let [entry] = entries(&path).try_into().expect("one entry");
        assert_eq!(entry["data"]["nested"]["key"], "value");
        assert_eq!(entry["data"]["number"], 42.5);
        assert_eq!(entry["data"]["bool"], true);
        assert!(entry["data"]["null_val"].is_null());
    }

    #[tokio::test]
    async fn a_caller_can_add_entries_of_its_own() {
        let directory = TempDir::new().expect("a temporary directory");
        let path = directory.path().join("events.jsonl");
        let journal = Journal::open(&path, "ENG-42", LIMIT).await;

        journal
            .append(
                "pr_url_extracted",
                json!({"pr_url": "https://github.com/org/repo/pull/1"}),
            )
            .await;

        let [entry] = entries(&path).try_into().expect("one entry");
        assert_eq!(entry["event"], "pr_url_extracted");
        assert_eq!(entry["label"], "ENG-42");
    }

    #[tokio::test]
    async fn line_entries_are_buffered_until_another_entry_flushes_them() {
        let directory = TempDir::new().expect("a temporary directory");
        let path = directory.path().join("events.jsonl");
        let journal = Journal::open(&path, "batched", LIMIT).await;

        journal
            .append_line(Record::StdoutLine, json!({"line": "one"}))
            .await;
        journal
            .append_line(Record::StdoutLine, json!({"line": "two"}))
            .await;
        assert!(
            std::fs::read_to_string(&path)
                .expect("the journal")
                .is_empty(),
            "a line entry was flushed on its own"
        );

        journal.append(Record::StdoutClosed, json!({})).await;
        let events: Vec<Value> = entries(&path)
            .into_iter()
            .map(|entry| entry["event"].clone())
            .collect();
        assert_eq!(
            events,
            ["stdout_line", "stdout_line", "stdout_stream_closed"]
        );
    }

    #[tokio::test]
    async fn line_entries_stop_at_the_limit_and_every_other_entry_goes_on() {
        let directory = TempDir::new().expect("a temporary directory");
        let path = directory.path().join("events.jsonl");
        let journal = Journal::open(&path, "capped", 2048).await;

        journal.append(Record::Spawned, json!({})).await;
        for number in 0..200 {
            journal
                .append_line(
                    Record::StdoutLine,
                    json!({"line_number": number, "line": "x".repeat(64)}),
                )
                .await;
        }
        journal.append(Record::Completed, json!({})).await;

        let size = std::fs::metadata(&path).expect("metadata").len();
        assert!(size < 4096, "the journal grew to {size} bytes");
        let events: Vec<String> = entries(&path)
            .into_iter()
            .filter_map(|entry| entry["event"].as_str().map(str::to_string))
            .collect();
        assert_eq!(
            events
                .iter()
                .filter(|event| *event == "journal_truncated")
                .count(),
            1
        );
        assert_eq!(
            events.first().map(String::as_str),
            Some("subprocess_spawned")
        );
        assert_eq!(events.last().map(String::as_str), Some("process_completed"));
        assert!(events.iter().any(|event| event == "stdout_line"));
    }

    #[tokio::test]
    async fn no_line_entry_follows_the_truncation_marker() {
        let directory = TempDir::new().expect("a temporary directory");
        let path = directory.path().join("events.jsonl");
        let journal = Journal::open(&path, "capped", 2048).await;

        journal
            .append_line(Record::StdoutLine, json!({"line": "x".repeat(4096)}))
            .await;
        for number in 0..5 {
            journal
                .append_line(Record::StdoutLine, json!({"line_number": number}))
                .await;
        }
        journal.append(Record::Completed, json!({})).await;

        let events: Vec<String> = entries(&path)
            .into_iter()
            .filter_map(|entry| entry["event"].as_str().map(str::to_string))
            .collect();
        assert_eq!(events, ["journal_truncated", "process_completed"]);
    }

    #[tokio::test]
    async fn a_journal_is_never_written_through_a_planted_link() {
        let directory = TempDir::new().expect("a temporary directory");
        let target = directory.path().join("target");
        std::fs::write(&target, b"untouched").expect("a target");
        let planted = directory.path().join("events.jsonl");
        std::os::unix::fs::symlink(&target, &planted).expect("a planted link");

        let journal = Journal::open(&planted, "linked", LIMIT).await;
        journal.append(Record::Initialized, json!({})).await;

        assert!(!journal.enabled());
        assert_eq!(
            std::fs::read_to_string(&target).expect("the target"),
            "untouched"
        );
    }

    #[tokio::test]
    async fn a_journal_that_cannot_be_opened_is_disabled() {
        let directory = TempDir::new().expect("a temporary directory");
        let journal =
            Journal::open(&directory.path().join("missing/events.jsonl"), "x", LIMIT).await;
        assert!(!journal.enabled());
    }
}
