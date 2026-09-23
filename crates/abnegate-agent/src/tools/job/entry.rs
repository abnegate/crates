use std::path::PathBuf;
use tokio::sync::{oneshot, watch};

use super::JobStatus;
use crate::tools::Session;

/// One entry in the registry [`Jobs`](super::Jobs) keeps.
pub(super) struct Job {
    pub(super) session: Session,
    pub(super) log: PathBuf,
    pub(super) state: watch::Receiver<JobStatus>,
    pub(super) kill: oneshot::Sender<()>,
}
