//! Job registry for tracking active and completed jobs.

use dashmap::DashMap;
use dashmap::mapref::entry::Entry;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::error::JobError;
use crate::executor::ProcessGroup;

use super::entry::JobEntry;
use super::state::JobState;

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

    /// Register a new job, leaving an existing job of the same identifier
    /// untouched.
    ///
    /// Returns the job's cancellation token. Spawn the job with
    /// [`CommandExecutor::spawn_with_cancellation`](crate::executor::CommandExecutor::spawn_with_cancellation)
    /// and this token, so that [`JobRegistry::cancel`] stops it.
    pub fn register(&self, job_id: String) -> Result<CancellationToken, JobError> {
        match self.jobs.entry(job_id) {
            Entry::Occupied(occupied) => Err(JobError::AlreadyExists(occupied.key().clone())),
            Entry::Vacant(vacant) => Ok(vacant.insert(JobEntry::new()).cancel_token()),
        }
    }

    /// Check if a job exists
    pub fn exists(&self, job_id: &str) -> bool {
        self.jobs.contains_key(job_id)
    }

    /// Get the current state of a job
    pub fn get_state(&self, job_id: &str) -> Option<JobState> {
        self.jobs.get(job_id).map(|entry| entry.state.clone())
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
    pub fn set_stdin(&self, job_id: &str, sender: mpsc::Sender<Vec<u8>>) -> Result<(), JobError> {
        match self.jobs.get_mut(job_id) {
            Some(mut entry) => {
                entry.stdin = Some(sender);
                Ok(())
            }
            None => Err(JobError::NotFound(job_id.to_string())),
        }
    }

    /// Get the stdin channel for a job
    pub fn get_stdin(&self, job_id: &str) -> Option<mpsc::Sender<Vec<u8>>> {
        self.jobs.get(job_id).and_then(|entry| entry.stdin.clone())
    }

    /// Close the stdin channel for a job
    pub fn close_stdin(&self, job_id: &str) {
        if let Some(mut entry) = self.jobs.get_mut(job_id) {
            entry.stdin = None;
        }
    }

    /// Get the cancellation token for a job
    pub fn get_cancel_token(&self, job_id: &str) -> Option<CancellationToken> {
        self.jobs.get(job_id).map(|entry| entry.cancel_token())
    }

    /// Cancel a job.
    ///
    /// If `force` is true, sends SIGKILL immediately; otherwise sends SIGTERM.
    pub fn cancel(&self, job_id: &str, force: bool) -> Result<(), JobError> {
        let entry = self
            .jobs
            .get(job_id)
            .ok_or_else(|| JobError::NotFound(job_id.to_string()))?;

        entry.cancellation.cancel();

        if let Some(group) = &entry.process_group {
            if force {
                let _ = group.kill();
            } else {
                let _ = group.terminate();
            }
        }

        Ok(())
    }

    /// Remove a job from the registry.
    ///
    /// Returns the entry if it existed.
    pub fn remove(&self, job_id: &str) -> Option<JobEntry> {
        self.jobs.remove(job_id).map(|(_, entry)| entry)
    }

    /// Cancel all jobs and clear the registry.
    pub fn cancel_all(&self) {
        for entry in self.jobs.iter() {
            entry.cancellation.cancel();
            if let Some(group) = &entry.process_group {
                let _ = group.kill();
            }
        }
        self.jobs.clear();
    }

    /// Get the number of active (non-terminal) jobs
    pub fn active_count(&self) -> usize {
        self.jobs
            .iter()
            .filter(|entry| !entry.state.is_terminal())
            .count()
    }

    /// Get the total number of tracked jobs
    pub fn total_count(&self) -> usize {
        self.jobs.len()
    }

    /// Get all job IDs
    pub fn job_ids(&self) -> Vec<String> {
        self.jobs.iter().map(|entry| entry.key().clone()).collect()
    }
}

impl Default for JobRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

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
        assert!(!token.is_cancelled());

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
            error => panic!("Wrong error: {error:?}"),
        }
    }

    #[test]
    fn a_duplicate_registration_leaves_the_first_job_in_place() {
        let registry = JobRegistry::new();
        let first = registry.register("job-1".to_string()).unwrap();
        registry
            .update_state("job-1", JobState::running(12345))
            .unwrap();

        assert!(registry.register("job-1".to_string()).is_err());
        registry.cancel("job-1", false).unwrap();

        assert!(
            first.is_cancelled(),
            "the registry still reaches the job that registered first"
        );
        assert!(registry.get_state("job-1").unwrap().is_running());
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
            error => panic!("Wrong error: {error:?}"),
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
            error => panic!("Wrong error: {error:?}"),
        }
    }

    #[tokio::test]
    async fn test_set_stdin() {
        let registry = JobRegistry::new();
        registry.register("job-1".to_string()).unwrap();

        let (sender, _receiver) = mpsc::channel::<Vec<u8>>(10);
        let result = registry.set_stdin("job-1", sender);
        assert!(result.is_ok());

        let stdin = registry.get_stdin("job-1");
        assert!(stdin.is_some());
    }

    #[tokio::test]
    async fn test_set_stdin_not_found() {
        let registry = JobRegistry::new();
        let (sender, _receiver) = mpsc::channel::<Vec<u8>>(10);
        let result = registry.set_stdin("nonexistent", sender);

        assert!(result.is_err());
        match result.unwrap_err() {
            JobError::NotFound(id) => assert_eq!(id, "nonexistent"),
            error => panic!("Wrong error: {error:?}"),
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

        let (sender, _receiver) = mpsc::channel::<Vec<u8>>(10);
        registry.set_stdin("job-1", sender).unwrap();

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

        registered_token.cancel();
        assert!(retrieved_token.is_cancelled());
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

        assert!(!token.is_cancelled());

        registry.cancel("job-1", false).unwrap();

        assert!(token.is_cancelled());
    }

    #[test]
    fn test_cancel_force() {
        let registry = JobRegistry::new();
        let token = registry.register("job-1".to_string()).unwrap();

        registry.cancel("job-1", true).unwrap();

        assert!(token.is_cancelled());
    }

    #[test]
    fn test_cancel_not_found() {
        let registry = JobRegistry::new();
        let result = registry.cancel("nonexistent", false);

        assert!(result.is_err());
        match result.unwrap_err() {
            JobError::NotFound(id) => assert_eq!(id, "nonexistent"),
            error => panic!("Wrong error: {error:?}"),
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
        let first = registry.register("job-1".to_string()).unwrap();
        let second = registry.register("job-2".to_string()).unwrap();

        registry.cancel_all();

        assert!(first.is_cancelled());
        assert!(second.is_cancelled());
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
