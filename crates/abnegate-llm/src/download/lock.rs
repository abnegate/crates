use std::fs::File;
use std::fs::TryLockError;
use std::path::Path;
use std::path::PathBuf;

use crate::download::error::DownloadError;

/// Sole use of one target's `.part` file and validator for a whole transfer.
///
/// An advisory lock on a `.lock` sibling, which the operating system drops
/// with the process, so a crash never leaves a target locked. The file is
/// removed, while still locked, when the transfer ends.
#[derive(Debug)]
pub(crate) struct TransferLock {
    path: PathBuf,
    _file: File,
}

impl TransferLock {
    pub(crate) fn path(part: &Path) -> PathBuf {
        let mut name = part.as_os_str().to_owned();
        name.push(".lock");
        PathBuf::from(name)
    }

    /// Take the lock for `part`, or [`DownloadError::InProgress`] when
    /// another transfer holds it.
    pub(crate) async fn acquire(part: &Path) -> Result<Self, DownloadError> {
        let path = Self::path(part);
        tokio::task::spawn_blocking(move || Self::acquire_blocking(path))
            .await
            .map_err(std::io::Error::other)?
    }

    fn acquire_blocking(path: PathBuf) -> Result<Self, DownloadError> {
        loop {
            let file = File::options()
                .create(true)
                .truncate(false)
                .write(true)
                .open(&path)?;
            match file.try_lock() {
                Ok(()) => {}
                Err(TryLockError::WouldBlock) => return Err(DownloadError::InProgress),
                Err(TryLockError::Error(error)) => return Err(error.into()),
            }
            if names(&file, &path) {
                return Ok(Self { path, _file: file });
            }
        }
    }
}

impl Drop for TransferLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Whether `path` still names `file`.
///
/// The holder before us removes the lock file before it unlocks it, so a
/// lock taken on a file opened just before that removal is a lock on a file
/// no one else will ever open, and must be taken again on whatever the path
/// names now.
#[cfg(unix)]
fn names(file: &File, path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;

    match (file.metadata(), std::fs::metadata(path)) {
        (Ok(held), Ok(named)) => held.dev() == named.dev() && held.ino() == named.ino(),
        _ => false,
    }
}

#[cfg(not(unix))]
fn names(_file: &File, _path: &Path) -> bool {
    true
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    #[tokio::test]
    async fn a_held_lock_refuses_a_second_holder_until_it_is_released() {
        let directory = tempdir().unwrap();
        let part = directory.path().join("model.gguf.part");

        let first = TransferLock::acquire(&part).await.unwrap();
        let second = TransferLock::acquire(&part).await;

        assert!(
            matches!(second, Err(DownloadError::InProgress)),
            "{second:?}"
        );
        drop(first);
        assert!(!TransferLock::path(&part).exists());
        TransferLock::acquire(&part).await.unwrap();
    }

    #[tokio::test]
    async fn a_lock_file_nobody_holds_is_taken_over() {
        let directory = tempdir().unwrap();
        let part = directory.path().join("model.gguf.part");
        std::fs::write(TransferLock::path(&part), b"").unwrap();

        TransferLock::acquire(&part).await.unwrap();
    }

    #[test]
    fn the_lock_lives_beside_the_part_file() {
        assert_eq!(
            TransferLock::path(Path::new("/models/model.gguf.part")),
            Path::new("/models/model.gguf.part.lock")
        );
    }
}
