use std::collections::HashMap;
use std::fmt::Display;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Models loaded once per weights file and shared by every caller after that.
///
/// A load runs on the blocking pool, since opening one is seconds of disk and
/// ONNX Runtime setup that would otherwise stall an async worker. Callers
/// asking for the same file wait for the one load in flight instead of
/// starting their own. A load that fails is not remembered, so weights put in
/// place or repaired later are picked up by the next caller.
pub(super) struct Models<T> {
    loaded: Mutex<HashMap<PathBuf, Arc<T>>>,
}

impl<T> Default for Models<T> {
    fn default() -> Self {
        Self {
            loaded: Mutex::new(HashMap::new()),
        }
    }
}

impl<T: Send + Sync + 'static> Models<T> {
    pub(super) async fn load<E, F>(&self, path: PathBuf, open: F) -> Option<Arc<T>>
    where
        E: Display + Send + 'static,
        F: FnOnce(&Path) -> Result<T, E> + Send + 'static,
    {
        let mut loaded = self.loaded.lock().await;
        if let Some(model) = loaded.get(&path) {
            return Some(Arc::clone(model));
        }
        let target = path.clone();
        match tokio::task::spawn_blocking(move || open(&target)).await {
            Ok(Ok(model)) => {
                tracing::info!(model = %path.display(), "training crops follow the subject");
                let model = Arc::new(model);
                loaded.insert(path, Arc::clone(&model));
                Some(model)
            }
            Ok(Err(error)) => {
                tracing::warn!(
                    model = %path.display(),
                    %error,
                    "subject detection is off; training crops fall back to the frame"
                );
                None
            }
            Err(error) => {
                tracing::warn!(
                    model = %path.display(),
                    %error,
                    "loading the subject model panicked; training crops fall back to the frame"
                );
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;
    use std::time::Duration;
    use std::time::Instant;

    fn weights() -> PathBuf {
        PathBuf::from("/models/vision/u2net.onnx")
    }

    #[tokio::test]
    async fn a_failed_load_is_retried_rather_than_remembered() {
        let models = Models::<u32>::default();
        assert_eq!(models.load(weights(), |_| Err("not there yet")).await, None);
        assert_eq!(
            models.load(weights(), |_| Ok::<u32, &str>(7)).await,
            Some(Arc::new(7))
        );
    }

    #[tokio::test]
    async fn a_loaded_model_is_opened_once_and_shared() {
        let models = Models::<u32>::default();
        let opened = Arc::new(AtomicUsize::new(0));
        for _ in 0..3 {
            let counter = Arc::clone(&opened);
            let model = models
                .load(weights(), move |_| {
                    counter.fetch_add(1, Ordering::SeqCst);
                    Ok::<u32, &str>(7)
                })
                .await;
            assert_eq!(model, Some(Arc::new(7)));
        }
        assert_eq!(opened.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_load_that_panics_leaves_the_cache_usable() {
        let models = Models::<u32>::default();
        assert_eq!(
            models
                .load(weights(), |_| -> Result<u32, &str> {
                    panic!("the runtime could not start")
                })
                .await,
            None
        );
        assert_eq!(
            models.load(weights(), |_| Ok::<u32, &str>(7)).await,
            Some(Arc::new(7))
        );
    }

    #[tokio::test]
    async fn a_slow_load_does_not_hold_the_async_runtime() {
        let models = Models::<u32>::default();
        let started = Instant::now();
        let (_, ticked) = tokio::join!(
            models.load(weights(), |_| {
                std::thread::sleep(Duration::from_millis(400));
                Ok::<u32, &str>(7)
            }),
            async {
                tokio::time::sleep(Duration::from_millis(10)).await;
                started.elapsed()
            }
        );
        assert!(
            ticked < Duration::from_millis(300),
            "the runtime was blocked for {ticked:?} by a model load"
        );
    }
}
