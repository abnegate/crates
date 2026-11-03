//! Work that blocks a thread, run on tokio's blocking pool.

/// Runs `work` on the blocking pool, so file reads and CPU-bound image work
/// never stall a runtime worker. A panic in `work` resumes in the caller.
/// `None` when the runtime shut down before `work` could run.
pub(crate) async fn run<T, F>(work: F) -> Option<T>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    match tokio::task::spawn_blocking(work).await {
        Ok(value) => Some(value),
        Err(error) if error.is_panic() => std::panic::resume_unwind(error.into_panic()),
        Err(_) => None,
    }
}
