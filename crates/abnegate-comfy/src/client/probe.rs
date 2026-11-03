//! Lets a test see which thread prepares a generation, and hold it there.

use std::path::Path;
use std::path::PathBuf;
use std::sync::Condvar;
use std::sync::Mutex;
use std::sync::PoisonError;
use std::thread::ThreadId;

static ENTERED: Mutex<Vec<(PathBuf, ThreadId)>> = Mutex::new(Vec::new());
static HELD: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
static RELEASED: Condvar = Condvar::new();

/// Records that this thread prepares a generation over `models`, then waits
/// while a test holds `models`.
pub(super) fn entered(models: &Path) {
    ENTERED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push((models.to_path_buf(), std::thread::current().id()));
    let mut held = HELD.lock().unwrap_or_else(PoisonError::into_inner);
    while held.iter().any(|path| path == models) {
        held = RELEASED.wait(held).unwrap_or_else(PoisonError::into_inner);
    }
}

/// The threads that prepared a generation over `models`.
pub(super) fn threads(models: &Path) -> Vec<ThreadId> {
    ENTERED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .filter(|(path, _)| path == models)
        .map(|(_, thread)| *thread)
        .collect()
}

/// Holds every generation over `models` in its preparation until
/// [`release`].
pub(super) fn hold(models: &Path) {
    HELD.lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(models.to_path_buf());
}

/// Lets the generations over `models` go on.
pub(super) fn release(models: &Path) {
    HELD.lock()
        .unwrap_or_else(PoisonError::into_inner)
        .retain(|path| path != models);
    RELEASED.notify_all();
}
