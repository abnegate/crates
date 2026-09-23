use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

const PERCENT: f64 = 100.0;

/// Live counters for one in-flight download.
#[derive(Debug, Default)]
pub struct DownloadProgress {
    pub downloaded_bytes: AtomicU64,
    pub total_bytes: AtomicU64,
    pub completed: AtomicBool,
    pub failed: AtomicBool,
    pub error_message: Mutex<Option<String>>,
}

impl DownloadProgress {
    pub fn new() -> Self {
        Self::default()
    }

    /// Completed fraction of the download, from 0 to 100.
    pub fn percent(&self) -> u8 {
        let total = self.total_bytes.load(Ordering::Relaxed);
        if total == 0 {
            return 0;
        }
        let downloaded = self.downloaded_bytes.load(Ordering::Relaxed);
        ((downloaded as f64 / total as f64) * PERCENT).min(PERCENT) as u8
    }

    pub(crate) fn fail(&self, message: String) {
        self.failed.store(true, Ordering::Relaxed);
        if let Ok(mut error) = self.error_message.lock() {
            *error = Some(message);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_tracks_downloaded_bytes() {
        let progress = DownloadProgress::new();
        assert_eq!(progress.percent(), 0);

        progress.total_bytes.store(1000, Ordering::Relaxed);
        progress.downloaded_bytes.store(500, Ordering::Relaxed);
        assert_eq!(progress.percent(), 50);

        progress.downloaded_bytes.store(1000, Ordering::Relaxed);
        assert_eq!(progress.percent(), 100);
    }

    #[test]
    fn percent_is_zero_without_a_total() {
        let progress = DownloadProgress::new();
        progress.downloaded_bytes.store(100, Ordering::Relaxed);
        assert_eq!(progress.percent(), 0);
    }

    #[test]
    fn default_starts_empty() {
        let progress = DownloadProgress::default();
        assert!(!progress.completed.load(Ordering::Relaxed));
        assert!(!progress.failed.load(Ordering::Relaxed));
        assert_eq!(progress.downloaded_bytes.load(Ordering::Relaxed), 0);
        assert_eq!(progress.total_bytes.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn fail_records_the_message() {
        let progress = DownloadProgress::new();
        progress.fail("boom".to_string());
        assert!(progress.failed.load(Ordering::Relaxed));
        assert_eq!(
            progress.error_message.lock().unwrap().as_deref(),
            Some("boom")
        );
    }
}
