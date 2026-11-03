use std::fs::File;
use std::io::ErrorKind;
use std::path::Path;
use std::path::PathBuf;

use crate::modality::vendor::fastembed::config::FastembedConfig;
use crate::provider::ProviderError;

const HUGGING_FACE_HOME: &str = "HF_HOME";
const LOCKS: &str = ".locks";

pub(super) fn directory(config: &FastembedConfig) -> PathBuf {
    std::env::var(HUGGING_FACE_HOME)
        .map(PathBuf::from)
        .ok()
        .or_else(|| config.cache_directory.clone())
        .unwrap_or_else(|| PathBuf::from(fastembed::get_cache_dir()))
}

pub(super) fn lock(directory: &Path, model_code: &str) -> Result<Option<File>, ProviderError> {
    let locks = directory.join(LOCKS);
    let path = locks.join(format!("models--{}.lock", model_code.replace('/', "--")));
    match acquire(&locks, &path) {
        Ok(file) => Ok(Some(file)),
        Err(error) if unlockable(&error) => {
            tracing::debug!(
                path = %path.display(),
                error = %error,
                "embedding cache cannot hold a lock, loading without one"
            );
            Ok(None)
        }
        Err(error) => Err(ProviderError::io(format!(
            "lock {}: {error}",
            path.display()
        ))),
    }
}

fn acquire(locks: &Path, path: &Path) -> std::io::Result<File> {
    std::fs::create_dir_all(locks)?;
    let file = File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)?;
    file.lock()?;
    Ok(file)
}

fn unlockable(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        ErrorKind::ReadOnlyFilesystem | ErrorKind::PermissionDenied | ErrorKind::Unsupported
    )
}

#[cfg(test)]
mod tests {
    use super::LOCKS;
    use super::lock;

    const MODEL_CODE: &str = "Qdrant/all-MiniLM-L6-v2-onnx";

    #[test]
    fn a_writable_cache_holds_the_lock_under_its_locks_directory() {
        let directory = tempfile::tempdir().unwrap();
        let held = lock(directory.path(), MODEL_CODE).unwrap();
        assert!(held.is_some());
        assert!(
            directory
                .path()
                .join(LOCKS)
                .join("models--Qdrant--all-MiniLM-L6-v2-onnx.lock")
                .is_file()
        );
        let entries: Vec<_> = std::fs::read_dir(directory.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(entries, [LOCKS]);
    }

    #[cfg(unix)]
    #[test]
    fn a_read_only_cache_loads_without_a_lock() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let cache = directory.path().join("cache");
        std::fs::create_dir(&cache).unwrap();
        std::fs::set_permissions(&cache, std::fs::Permissions::from_mode(0o555)).unwrap();
        let enforced = std::fs::create_dir(cache.join("probe")).is_err();
        let existing = lock(&cache, MODEL_CODE);
        let missing = lock(&cache.join("missing"), MODEL_CODE);
        std::fs::set_permissions(&cache, std::fs::Permissions::from_mode(0o755)).unwrap();
        if !enforced {
            eprintln!("skipped: permissions are not enforced for this user");
            return;
        }
        assert!(existing.unwrap().is_none());
        assert!(missing.unwrap().is_none());
    }

    #[test]
    fn a_cache_that_is_a_file_fails_to_lock() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("cache");
        std::fs::write(&file, b"").unwrap();
        let error = lock(&file, MODEL_CODE).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("models--Qdrant--all-MiniLM-L6-v2-onnx.lock"),
            "lost the path: {error}"
        );
    }
}
