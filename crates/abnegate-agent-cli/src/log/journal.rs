use std::fmt;
use std::path::Path;
use std::sync::Arc;

use serde_json::json;
use tokio::fs::File;
use tokio::fs::OpenOptions;
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;

/// An append-only JSONL account of one run, shared by the tasks that drive it.
///
/// A disabled journal accepts every entry and writes none, so a caller never
/// branches on whether logging is on. The provider's own entries are named by
/// [`Record`](crate::log::Record); a caller can add its own under names of its
/// choosing to the same file.
#[derive(Debug, Clone, Default)]
pub struct Journal {
    label: String,
    file: Option<Arc<Mutex<File>>>,
}

impl Journal {
    pub fn disabled() -> Self {
        Self::default()
    }

    /// Append to the journal at `path`, or a disabled journal when it cannot
    /// be opened.
    pub async fn open(path: &Path, label: impl Into<String>) -> Self {
        let label = label.into();
        match OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .await
        {
            Ok(file) => Self {
                label,
                file: Some(Arc::new(Mutex::new(file))),
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
        self.file.is_some()
    }

    /// Append one entry. Each entry is a single write of a whole line, so
    /// concurrent writers never interleave within one.
    pub async fn append(&self, event: impl fmt::Display, data: serde_json::Value) {
        let Some(file) = &self.file else {
            return;
        };

        let entry = json!({
            "timestamp": chrono::Utc::now().to_rfc3339(),
            "label": self.label,
            "event": event.to_string(),
            "data": data,
        });
        let mut line = match serde_json::to_vec(&entry) {
            Ok(line) => line,
            Err(error) => {
                tracing::warn!(label = %self.label, %event, %error, "could not serialise a journal entry");
                return;
            }
        };
        line.push(b'\n');

        let mut file = file.lock().await;
        let written = match file.write_all(&line).await {
            Ok(()) => file.flush().await,
            Err(error) => Err(error),
        };
        if let Err(error) = written {
            tracing::warn!(label = %self.label, %event, %error, "could not write a journal entry");
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
        let journal = Journal::open(&path, "my-label").await;
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
    async fn entries_are_appended_in_order() {
        let directory = TempDir::new().expect("a temporary directory");
        let path = directory.path().join("multi_events.jsonl");
        let journal = Journal::open(&path, "lbl").await;

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
        let journal = Journal::open(&path, "nested").await;

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
        let journal = Journal::open(&path, "ENG-42").await;

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
    async fn a_journal_that_cannot_be_opened_is_disabled() {
        let directory = TempDir::new().expect("a temporary directory");
        let journal = Journal::open(&directory.path().join("missing/events.jsonl"), "x").await;
        assert!(!journal.enabled());
    }
}
