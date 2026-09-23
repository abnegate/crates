use std::path::Path;

use tokio::fs::File;
use tokio::fs::OpenOptions;
use tokio::io::AsyncWriteExt;

/// A best-effort raw log file. The first write that fails closes it, so one
/// full disk costs a warning rather than the run.
#[derive(Debug, Default)]
pub(crate) struct Sink {
    file: Option<File>,
}

impl Sink {
    pub(crate) async fn open(path: Option<&Path>) -> Self {
        let Some(path) = path else {
            return Self::default();
        };
        match OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .await
        {
            Ok(file) => Self { file: Some(file) },
            Err(error) => {
                tracing::warn!(path = %path.display(), %error, "could not open an execution log file");
                Self::default()
            }
        }
    }

    pub(crate) async fn write(&mut self, bytes: &[u8]) {
        let Some(file) = &mut self.file else {
            return;
        };
        if let Err(error) = file.write_all(bytes).await {
            tracing::warn!(%error, "could not write an execution log file; closing it");
            self.file = None;
        }
    }

    /// Wait for every write to reach the file.
    pub(crate) async fn finish(&mut self) {
        if let Some(file) = &mut self.file
            && let Err(error) = file.flush().await
        {
            tracing::warn!(%error, "could not flush an execution log file");
        }
    }
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::Sink;

    #[tokio::test]
    async fn writes_reach_the_file_once_finished() {
        let directory = TempDir::new().expect("a temporary directory");
        let path = directory.path().join("run.stdout.log");

        let mut sink = Sink::open(Some(&path)).await;
        sink.write(b"hello ").await;
        sink.write(b"world").await;
        sink.finish().await;

        assert_eq!(
            std::fs::read_to_string(&path).expect("the log"),
            "hello world"
        );
    }

    #[tokio::test]
    async fn a_sink_without_a_file_swallows_writes() {
        let mut sink = Sink::open(None).await;
        sink.write(b"ignored").await;
        sink.finish().await;

        let directory = TempDir::new().expect("a temporary directory");
        let mut sink = Sink::open(Some(&directory.path().join("missing/run.log"))).await;
        sink.write(b"ignored").await;
        sink.finish().await;
    }
}
