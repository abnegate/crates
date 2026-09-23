use std::io;

use nix::sys::wait::Id;
use nix::sys::wait::WaitPidFlag;
use nix::sys::wait::WaitStatus;
use nix::sys::wait::waitid;
use nix::unistd::Pid;
use tokio::signal::unix::Signal;
use tokio::signal::unix::SignalKind;
use tokio::signal::unix::signal;

/// Resolves once a child has exited, leaving it unreaped.
pub(in crate::executor) struct LeaderExit {
    pid: Pid,
    signals: Signal,
    exited: bool,
}

impl LeaderExit {
    pub(in crate::executor) fn watch(pid: Pid) -> io::Result<Self> {
        Ok(Self {
            pid,
            signals: signal(SignalKind::child())?,
            exited: false,
        })
    }

    pub(in crate::executor) async fn wait(&mut self) -> io::Result<()> {
        while !self.has_exited()? {
            if self.signals.recv().await.is_none() {
                return Err(io::Error::other("the SIGCHLD stream ended"));
            }
        }
        Ok(())
    }

    fn has_exited(&mut self) -> io::Result<bool> {
        if !self.exited {
            let status = waitid(
                Id::Pid(self.pid),
                WaitPidFlag::WEXITED | WaitPidFlag::WNOHANG | WaitPidFlag::WNOWAIT,
            )?;
            self.exited = !matches!(status, WaitStatus::StillAlive);
        }
        Ok(self.exited)
    }
}
