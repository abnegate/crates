use std::sync::Arc;

use tokio::sync::oneshot;
use tokio::sync::watch;

use super::JobStatus;
use super::log::Log;
use crate::tools::Session;

/// One entry in the registry [`Jobs`](super::Jobs) keeps.
pub(super) struct Job {
    pub(super) session: Session,
    pub(super) log: Arc<Log>,
    pub(super) state: watch::Receiver<JobStatus>,
    pub(super) kill: oneshot::Sender<()>,
}
