use nix::errno::Errno;
use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;

/// A child started as the leader of its own process group, and everything it
/// went on to start.
///
/// Killing only the child leaves its children running with the pipes and
/// files it shared still open, so every kill here goes to the whole group,
/// and dropping the handle is a kill: a call abandoned by a timeout leaves
/// nothing behind.
#[derive(Debug)]
pub(crate) struct Group {
    leader: Option<Pid>,
}

impl Group {
    /// The group led by `id`, which has to have been spawned with
    /// `process_group(0)`.
    pub(crate) fn led_by(id: Option<u32>) -> Self {
        Self {
            leader: id
                .and_then(|id| i32::try_from(id).ok())
                .filter(|id| *id > 0)
                .map(Pid::from_raw),
        }
    }

    /// Kill every process still in the group. Later calls do nothing.
    pub(crate) fn kill(&mut self) {
        if let Some(leader) = self.leader.take() {
            match killpg(leader, Signal::SIGKILL) {
                Ok(()) | Err(Errno::ESRCH) => {}
                Err(error) => {
                    tracing::warn!(group = leader.as_raw(), %error, "Could not kill a process group");
                }
            }
        }
    }
}

impl Drop for Group {
    fn drop(&mut self) {
        self.kill();
    }
}
