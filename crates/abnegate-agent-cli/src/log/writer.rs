use tokio::fs::File;
use tokio::io::AsyncWriteExt;
use tokio::io::BufWriter;

/// A journal's file and how much has gone into it.
#[derive(Debug)]
pub(crate) struct Writer {
    file: BufWriter<File>,
    written: u64,
    limit: u64,
    truncated: bool,
}

impl Writer {
    pub(crate) fn new(file: File, limit: u64) -> Self {
        Self {
            file: BufWriter::new(file),
            written: 0,
            limit,
            truncated: false,
        }
    }

    /// Whether `bytes` more would take the file past its limit.
    pub(crate) fn full(&self, bytes: usize) -> bool {
        self.written
            .saturating_add(u64::try_from(bytes).unwrap_or(u64::MAX))
            > self.limit
    }

    /// Mark the file truncated, and say whether it already was.
    pub(crate) fn truncate(&mut self) -> bool {
        std::mem::replace(&mut self.truncated, true)
    }

    pub(crate) fn limit(&self) -> u64 {
        self.limit
    }

    pub(crate) async fn write(&mut self, line: &[u8]) -> std::io::Result<()> {
        self.file.write_all(line).await?;
        self.written = self
            .written
            .saturating_add(u64::try_from(line.len()).unwrap_or(u64::MAX));
        Ok(())
    }

    pub(crate) async fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush().await
    }
}
