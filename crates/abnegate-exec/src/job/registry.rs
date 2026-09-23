//! Job registry for tracking active and completed jobs.

use dashmap::DashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;
use tokio::sync::mpsc;

use crate::error::JobError;
use crate::executor::ProcessGroup;

use super::state::JobState;

/// Entry for a tracked job
pub struct JobEntry {
    /// Current state of the job
    pub state: JobState,

    /// Cancellation flag
    pub cancelled: Arc<AtomicBool>,

    /// Process group for signaling (if running)
    pub process_group: Option<ProcessGroup>,

    /// Channel to send stdin data
    pub stdin_tx: Option<mpsc::Sender<Vec<u8>>>,

    /// When the job was registered
    pub created_at: Instant,
}

impl JobEntry {
    /// Create a new job entry
    pub fn new() -> Self {
        Self {
            state: JobState::new(),
            cancelled: Arc::new(AtomicBool::new(false)),
            process_group: None,
            stdin_tx: None,
            created_at: Instant::now(),
        }
    }

    /// Get a clone of the cancellation flag
    pub fn cancel_token(&self) -> Arc<AtomicBool> {
        self.cancelled.clone()
    }
}

impl Default for JobEntry {
    fn default() -> Self {
        Self::new()
    }
}

/// Thread-safe registry for tracking jobs.
pub struct JobRegistry {
    jobs: DashMap<String, JobEntry>,
}

impl JobRegistry {
    /// Create a new empty registry
    pub fn new() -> Self {
        Self {
            jobs: DashMap::new(),
        }
    }

    /// Register a new job.
    ///
    /// Returns the cancellation token if successful.
    pub fn register(&self, job_id: String) -> Result<Arc<AtomicBool>, JobError> {
        let entry = JobEntry::new();
        let cancel_token = entry.cancel_token();

        if self.jobs.insert(job_id.clone(), entry).is_some() {
            return Err(JobError::AlreadyExists(job_id));
        }

        Ok(cancel_token)
    }

    /// Check if a job exists
    pub fn exists(&self, job_id: &str) -> bool {
        self.jobs.contains_key(job_id)
    }

    /// Get the current state of a job
    pub fn get_state(&self, job_id: &str) -> Option<JobState> {
        self.jobs.get(job_id).map(|e| e.state.clone())
    }

    /// Update the state of a job
    pub fn update_state(&self, job_id: &str, state: JobState) -> Result<(), JobError> {
        match self.jobs.get_mut(job_id) {
            Some(mut entry) => {
                entry.state = state;
                Ok(())
            }
            None => Err(JobError::NotFound(job_id.to_string())),
        }
    }

    /// Set the process group for a job
    pub fn set_process_group(
        &self,
        job_id: &str,
        process_group: ProcessGroup,
    ) -> Result<(), JobError> {
        match self.jobs.get_mut(job_id) {
            Some(mut entry) => {
                entry.process_group = Some(process_group);
                Ok(())
            }
            None => Err(JobError::NotFound(job_id.to_string())),
        }
    }

    /// Set the stdin channel for a job
    pub fn set_stdin(&self, job_id: &str, tx: mpsc::Sender<Vec<u8>>) -> Result<(), JobError> {
        match self.jobs.get_mut(job_id) {
            Some(mut entry) => {
                entry.stdin_tx = Some(tx);
                Ok(())
            }
            None => Err(JobError::NotFound(job_id.to_string())),
        }
    }

    /// Get the stdin channel for a job
    pub fn get_stdin(&self, job_id: &str) -> Option<mpsc::Sender<Vec<u8>>> {
        self.jobs.get(job_id).and_then(|e| e.stdin_tx.clone())
    }

    /// Close the stdin channel for a job
    pub fn close_stdin(&self, job_id: &str) {
        if let Some(mut entry) = self.jobs.get_mut(job_id) {
            entry.stdin_tx = None;
        }
    }

    /// Get the cancellation token for a job
    pub fn get_cancel_token(&self, job_id: &str) -> Option<Arc<AtomicBool>> {
        self.jobs.get(job_id).map(|e| e.cancel_token())
    }

    /// Cancel a job.
    ///
    /// If `force` is true, sends SIGKILL immediately; otherwise sends SIGTERM.
    pub fn cancel(&self, job_id: &str, force: bool) -> Result<(), JobError> {
        let entry = self
            .jobs
            .get(job_id)
            .ok_or_else(|| JobError::NotFound(job_id.to_string()))?;

        entry.cancelled.store(true, Ordering::SeqCst);

        if let Some(ref pg) = entry.process_group {
            if force {
                let _ = pg.kill();
            } else {
                let _ = pg.terminate();
            }
        }

        Ok(())
    }

    /// Remove a job from the registry.
    ///
    /// Returns the entry if it existed.
    pub fn remove(&self, job_id: &str) -> Option<JobEntry> {
        self.jobs.remove(job_id).map(|(_, v)| v)
    }

    /// Cancel all jobs and clear the registry.
    pub fn cancel_all(&self) {
        for entry in self.jobs.iter() {
            entry.cancelled.store(true, Ordering::SeqCst);
            if let Some(ref pg) = entry.process_group {
                let _ = pg.kill();
            }
        }
        self.jobs.clear();
    }

    /// Get the number of active (non-terminal) jobs
    pub fn active_count(&self) -> usize {
        self.jobs.iter().filter(|e| !e.state.is_terminal()).count()
    }

    /// Get the total number of tracked jobs
    pub fn total_count(&self) -> usize {
        self.jobs.len()
    }

    /// Get all job IDs
    pub fn job_ids(&self) -> Vec<String> {
        self.jobs.iter().map(|e| e.key().clone()).collect()
    }
}

impl Default for JobRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_job_entry_new() {
        let entry = JobEntry::new();
        assert!(!entry.state.is_terminal());
        assert!(!entry.cancelled.load(Ordering::SeqCst));
        assert!(entry.process_group.is_none());
        assert!(entry.stdin_tx.is_none());
    }

    #[test]
    fn test_job_entry_default() {
        let entry: JobEntry = Default::default();
        assert!(!entry.state.is_terminal());
        assert!(!entry.cancelled.load(Ordering::SeqCst));
    }

    #[test]
    fn test_job_entry_cancel_token() {
        let entry = JobEntry::new();
        let token1 = entry.cancel_token();
        let token2 = entry.cancel_token();

        token1.store(true, Ordering::SeqCst);
        assert!(token2.load(Ordering::SeqCst));
    }

    #[test]
    fn test_registry_new() {
        let registry = JobRegistry::new();
        assert_eq!(registry.total_count(), 0);
        assert_eq!(registry.active_count(), 0);
    }

    #[test]
    fn test_registry_default() {
        let registry: JobRegistry = Default::default();
        assert_eq!(registry.total_count(), 0);
    }

    #[test]
    fn test_register_and_get() {
        let registry = JobRegistry::new();

        let token = registry.register("job-1".to_string()).unwrap();
        assert!(!token.load(Ordering::SeqCst));

        assert!(registry.exists("job-1"));
        assert!(!registry.exists("job-2"));

        let state = registry.get_state("job-1").unwrap();
        assert!(!state.is_terminal());
    }

    #[test]
    fn test_duplicate_registration() {
        let registry = JobRegistry::new();

        registry.register("job-1".to_string()).unwrap();
        let result = registry.register("job-1".to_string());

        assert!(result.is_err());
        match result.unwrap_err() {
            JobError::AlreadyExists(id) => assert_eq!(id, "job-1"),
            e => panic!("Wrong error: {:?}", e),
        }
    }

    #[test]
    fn test_update_state() {
        let registry = JobRegistry::new();
        registry.register("job-1".to_string()).unwrap();

        registry
            .update_state("job-1", JobState::running(12345))
            .unwrap();

        let state = registry.get_state("job-1").unwrap();
        assert!(state.is_running());
        assert_eq!(state.pid(), Some(12345));
    }

    #[test]
    fn test_update_state_not_found() {
        let registry = JobRegistry::new();
        let result = registry.update_state("nonexistent", JobState::running(123));

        assert!(result.is_err());
        match result.unwrap_err() {
            JobError::NotFound(id) => assert_eq!(id, "nonexistent"),
            e => panic!("Wrong error: {:?}", e),
        }
    }

    #[test]
    fn test_get_state_not_found() {
        let registry = JobRegistry::new();
        assert!(registry.get_state("nonexistent").is_none());
    }

    #[test]
    fn test_set_process_group() {
        let registry = JobRegistry::new();
        registry.register("job-1".to_string()).unwrap();

        let pg = ProcessGroup::new(12345);
        let result = registry.set_process_group("job-1", pg);
        assert!(result.is_ok());
    }

    #[test]
    fn test_set_process_group_not_found() {
        let registry = JobRegistry::new();
        let pg = ProcessGroup::new(12345);
        let result = registry.set_process_group("nonexistent", pg);

        assert!(result.is_err());
        match result.unwrap_err() {
            JobError::NotFound(id) => assert_eq!(id, "nonexistent"),
            e => panic!("Wrong error: {:?}", e),
        }
    }

    #[tokio::test]
    async fn test_set_stdin() {
        let registry = JobRegistry::new();
        registry.register("job-1".to_string()).unwrap();

        let (tx, _rx) = mpsc::channel::<Vec<u8>>(10);
        let result = registry.set_stdin("job-1", tx);
        assert!(result.is_ok());

        let stdin = registry.get_stdin("job-1");
        assert!(stdin.is_some());
    }

    #[tokio::test]
    async fn test_set_stdin_not_found() {
        let registry = JobRegistry::new();
        let (tx, _rx) = mpsc::channel::<Vec<u8>>(10);
        let result = registry.set_stdin("nonexistent", tx);

        assert!(result.is_err());
        match result.unwrap_err() {
            JobError::NotFound(id) => assert_eq!(id, "nonexistent"),
            e => panic!("Wrong error: {:?}", e),
        }
    }

    #[tokio::test]
    async fn test_get_stdin_none() {
        let registry = JobRegistry::new();
        registry.register("job-1".to_string()).unwrap();

        let stdin = registry.get_stdin("job-1");
        assert!(stdin.is_none());
    }

    #[tokio::test]
    async fn test_get_stdin_not_found() {
        let registry = JobRegistry::new();
        let stdin = registry.get_stdin("nonexistent");
        assert!(stdin.is_none());
    }

    #[tokio::test]
    async fn test_close_stdin() {
        let registry = JobRegistry::new();
        registry.register("job-1".to_string()).unwrap();

        let (tx, _rx) = mpsc::channel::<Vec<u8>>(10);
        registry.set_stdin("job-1", tx).unwrap();

        assert!(registry.get_stdin("job-1").is_some());

        registry.close_stdin("job-1");

        assert!(registry.get_stdin("job-1").is_none());
    }

    #[tokio::test]
    async fn test_close_stdin_nonexistent_job() {
        let registry = JobRegistry::new();
        registry.close_stdin("nonexistent");
    }

    #[test]
    fn test_get_cancel_token() {
        let registry = JobRegistry::new();
        let registered_token = registry.register("job-1".to_string()).unwrap();

        let retrieved_token = registry.get_cancel_token("job-1").unwrap();

        registered_token.store(true, Ordering::SeqCst);
        assert!(retrieved_token.load(Ordering::SeqCst));
    }

    #[test]
    fn test_get_cancel_token_not_found() {
        let registry = JobRegistry::new();
        assert!(registry.get_cancel_token("nonexistent").is_none());
    }

    #[test]
    fn test_cancel() {
        let registry = JobRegistry::new();
        let token = registry.register("job-1".to_string()).unwrap();

        assert!(!token.load(Ordering::SeqCst));

        registry.cancel("job-1", false).unwrap();

        assert!(token.load(Ordering::SeqCst));
    }

    #[test]
    fn test_cancel_force() {
        let registry = JobRegistry::new();
        let token = registry.register("job-1".to_string()).unwrap();

        registry.cancel("job-1", true).unwrap();

        assert!(token.load(Ordering::SeqCst));
    }

    #[test]
    fn test_cancel_not_found() {
        let registry = JobRegistry::new();
        let result = registry.cancel("nonexistent", false);

        assert!(result.is_err());
        match result.unwrap_err() {
            JobError::NotFound(id) => assert_eq!(id, "nonexistent"),
            e => panic!("Wrong error: {:?}", e),
        }
    }

    #[test]
    fn test_cancel_with_process_group() {
        let registry = JobRegistry::new();
        registry.register("job-1".to_string()).unwrap();

        let pg = ProcessGroup::new(999999);
        registry.set_process_group("job-1", pg).unwrap();

        let result = registry.cancel("job-1", false);
        assert!(result.is_ok());
    }

    #[test]
    fn test_cancel_force_with_process_group() {
        let registry = JobRegistry::new();
        registry.register("job-1".to_string()).unwrap();

        let pg = ProcessGroup::new(999999);
        registry.set_process_group("job-1", pg).unwrap();

        let result = registry.cancel("job-1", true);
        assert!(result.is_ok());
    }

    #[test]
    fn test_remove() {
        let registry = JobRegistry::new();
        registry.register("job-1".to_string()).unwrap();

        assert!(registry.exists("job-1"));
        let entry = registry.remove("job-1");
        assert!(entry.is_some());
        assert!(!registry.exists("job-1"));
    }

    #[test]
    fn test_remove_nonexistent() {
        let registry = JobRegistry::new();
        let entry = registry.remove("nonexistent");
        assert!(entry.is_none());
    }

    #[test]
    fn test_cancel_all() {
        let registry = JobRegistry::new();
        let t1 = registry.register("job-1".to_string()).unwrap();
        let t2 = registry.register("job-2".to_string()).unwrap();

        registry.cancel_all();

        assert!(t1.load(Ordering::SeqCst));
        assert!(t2.load(Ordering::SeqCst));
        assert_eq!(registry.total_count(), 0);
    }

    #[test]
    fn test_cancel_all_empty() {
        let registry = JobRegistry::new();
        registry.cancel_all();
        assert_eq!(registry.total_count(), 0);
    }

    #[test]
    fn test_cancel_all_with_process_groups() {
        let registry = JobRegistry::new();
        registry.register("job-1".to_string()).unwrap();
        registry.register("job-2".to_string()).unwrap();

        let pg1 = ProcessGroup::new(999998);
        let pg2 = ProcessGroup::new(999999);
        registry.set_process_group("job-1", pg1).unwrap();
        registry.set_process_group("job-2", pg2).unwrap();

        registry.cancel_all();
        assert_eq!(registry.total_count(), 0);
    }

    #[test]
    fn test_active_count() {
        let registry = JobRegistry::new();
        registry.register("job-1".to_string()).unwrap();
        registry.register("job-2".to_string()).unwrap();
        registry.register("job-3".to_string()).unwrap();

        assert_eq!(registry.active_count(), 3);

        registry
            .update_state("job-1", JobState::completed(0, Duration::from_secs(1)))
            .unwrap();

        assert_eq!(registry.active_count(), 2);
    }

    #[test]
    fn test_total_count() {
        let registry = JobRegistry::new();
        assert_eq!(registry.total_count(), 0);

        registry.register("job-1".to_string()).unwrap();
        assert_eq!(registry.total_count(), 1);

        registry.register("job-2".to_string()).unwrap();
        assert_eq!(registry.total_count(), 2);

        registry.remove("job-1");
        assert_eq!(registry.total_count(), 1);
    }

    #[test]
    fn test_job_ids() {
        let registry = JobRegistry::new();
        registry.register("job-a".to_string()).unwrap();
        registry.register("job-b".to_string()).unwrap();
        registry.register("job-c".to_string()).unwrap();

        let mut ids = registry.job_ids();
        ids.sort();

        assert_eq!(ids, vec!["job-a", "job-b", "job-c"]);
    }

    #[test]
    fn test_job_ids_empty() {
        let registry = JobRegistry::new();
        let ids = registry.job_ids();
        assert!(ids.is_empty());
    }

    #[test]
    fn test_exists_after_remove() {
        let registry = JobRegistry::new();
        registry.register("job-1".to_string()).unwrap();

        assert!(registry.exists("job-1"));
        registry.remove("job-1");
        assert!(!registry.exists("job-1"));
    }

    #[test]
    fn test_multiple_state_updates() {
        let registry = JobRegistry::new();
        registry.register("job-1".to_string()).unwrap();

        registry
            .update_state("job-1", JobState::running(100))
            .unwrap();
        assert!(registry.get_state("job-1").unwrap().is_running());

        registry
            .update_state("job-1", JobState::completed(0, Duration::from_secs(1)))
            .unwrap();
        assert!(registry.get_state("job-1").unwrap().is_terminal());
    }
}
