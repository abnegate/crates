use std::io;
use std::process::ExitStatus;

use nix::sys::wait::Id;
use nix::sys::wait::WaitPidFlag;
use nix::sys::wait::waitid;
use nix::unistd::Pid;

/// Checks whether a child has exited, leaving it unreaped, and keeps the
/// status it exited with.
pub(super) struct Probe {
    pid: Pid,
    status: Option<ExitStatus>,
}

impl Probe {
    pub(super) fn watch(pid: Pid) -> io::Result<Self> {
        Ok(Self { pid, status: None })
    }

    /// The status the child exited with, or `None` while it runs.
    pub(super) fn exit(&mut self) -> io::Result<Option<ExitStatus>> {
        if self.status.is_none() {
            self.status = super::reported(super::uninterrupted(|| {
                waitid(
                    Id::Pid(self.pid),
                    WaitPidFlag::WEXITED | WaitPidFlag::WNOHANG | WaitPidFlag::WNOWAIT,
                )
            })?);
        }
        Ok(self.status)
    }
}
