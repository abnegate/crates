use std::fs::File;
use std::io::BufWriter;
use std::io::Write as _;
use std::sync::Arc;

use tokio::sync::mpsc::Receiver;
use tokio::sync::mpsc::Sender;
use tokio::sync::mpsc::channel;
use tokio::task::JoinHandle;

use crate::download::lock::TransferLock;

/// How many chunks may wait for the disk before the download stops reading
/// from the network.
const QUEUED_CHUNKS: usize = 16;

/// Appends a body to the `.part` file from one blocking thread that shares
/// the transfer's lock.
///
/// The lock outlives the last write, so a transfer that fails or is dropped
/// mid-body never has a write land on the `.part` file after another
/// transfer has taken it. A dropped writer still writes what it had queued.
pub(crate) struct Writer<Chunk> {
    chunks: Sender<Chunk>,
    task: JoinHandle<std::io::Result<()>>,
}

impl<Chunk: AsRef<[u8]> + Send + 'static> Writer<Chunk> {
    pub(crate) fn spawn(file: File, lock: &Arc<TransferLock>) -> Self {
        let (chunks, queue) = channel(QUEUED_CHUNKS);
        let task = lock.hold(move || append(file, queue));
        Self { chunks, task }
    }

    /// Queue `chunk`, or `false` once the writer has stopped, which
    /// [`Writer::finish`] then reports.
    pub(crate) async fn write(&self, chunk: Chunk) -> bool {
        self.chunks.send(chunk).await.is_ok()
    }

    /// Wait until every queued chunk is on disk.
    pub(crate) async fn finish(self) -> std::io::Result<()> {
        drop(self.chunks);
        self.task.await.map_err(std::io::Error::other)?
    }
}

fn append<Chunk: AsRef<[u8]>>(file: File, mut queue: Receiver<Chunk>) -> std::io::Result<()> {
    let mut file = BufWriter::new(file);
    while let Some(chunk) = queue.blocking_recv() {
        file.write_all(chunk.as_ref())?;
    }
    file.flush()
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use tempfile::tempdir;

    use super::*;

    async fn locked(part: &Path) -> Arc<TransferLock> {
        Arc::new(TransferLock::acquire(part).await.unwrap())
    }

    #[tokio::test]
    async fn every_queued_chunk_is_on_disk_once_the_writer_finishes() {
        let directory = tempdir().unwrap();
        let part = directory.path().join("model.gguf.part");
        let writer = Writer::spawn(File::create(&part).unwrap(), &locked(&part).await);

        for chunk in [b"gguf".as_slice(), b"-", b"body"] {
            assert!(writer.write(chunk).await);
        }
        writer.finish().await.unwrap();

        assert_eq!(std::fs::read(&part).unwrap(), b"gguf-body");
    }

    #[tokio::test]
    async fn a_dropped_writer_keeps_the_lock_until_its_queue_is_written() {
        let directory = tempdir().unwrap();
        let part = directory.path().join("model.gguf.part");
        let chunk = vec![7_u8; 64 << 10];
        let writer = Writer::spawn(File::create(&part).unwrap(), &locked(&part).await);

        for _ in 0..QUEUED_CHUNKS {
            assert!(writer.write(chunk.clone()).await);
        }
        drop(writer);
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while TransferLock::path(&part).exists() {
                tokio::time::sleep(std::time::Duration::from_millis(1)).await;
            }
        })
        .await
        .expect("the writer released the lock");

        assert_eq!(
            std::fs::metadata(&part).unwrap().len(),
            (QUEUED_CHUNKS * chunk.len()) as u64
        );
        TransferLock::acquire(&part).await.unwrap();
    }

    #[tokio::test]
    async fn a_failed_write_is_reported_when_the_writer_finishes() {
        let directory = tempdir().unwrap();
        let part = directory.path().join("model.gguf.part");
        std::fs::write(&part, b"").unwrap();
        let writer = Writer::spawn(File::open(&part).unwrap(), &locked(&part).await);

        writer.write(vec![0_u8; 64 << 10]).await;

        assert!(writer.finish().await.is_err());
        TransferLock::acquire(&part).await.unwrap();
    }
}
