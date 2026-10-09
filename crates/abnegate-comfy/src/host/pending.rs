use crate::host::JobDirectory;

/// A job directory being filled for the host trainer.
///
/// Dropped before [`Pending::queued`], which is any failure or cancellation
/// before `job.json` is written, it removes the whole directory, secrets
/// included. Dropped after, it removes the secrets the job lists once the job
/// has ended, and leaves a job the trainer may still hold, with its secrets,
/// to the trainer and to [`resume`](crate::host::resume) or
/// [`recover`](crate::host::recover).
pub(crate) struct Pending {
    directory: JobDirectory,
    queued: bool,
}

impl Pending {
    pub(crate) fn new(directory: JobDirectory) -> Self {
        Self {
            directory,
            queued: false,
        }
    }

    /// Marks `job.json` written, so the directory is the trainer's now.
    pub(crate) fn queued(&mut self) {
        self.queued = true;
    }
}

impl Drop for Pending {
    fn drop(&mut self) {
        if !self.queued {
            let _ = self.directory.discard();
            return;
        }
        if let Ok(job) = self.directory.read_job()
            && !job.busy()
        {
            self.directory.remove_job_secrets(&job);
        }
    }
}
