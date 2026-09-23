//! Process group management for clean process tree termination.
//!
//! On Unix systems, we create a new process group for each spawned command,
//! allowing us to send signals to the entire process tree when cancelling.

use std::time::Duration;

use nix::errno::Errno;
use nix::sys::signal::Signal;
use nix::sys::signal::kill;
use nix::unistd::Pid;
use nix::unistd::getpgrp;
use tokio::time::sleep;

use crate::error::ExecutorError;

/// The lowest process identifier that can lead a job's group. `0` names the
/// caller's own group and `1` is init, and `kill(-1, ...)` signals every
/// process the caller may signal.
const LOWEST_GROUP: u32 = 2;

/// A handle to a process group for signal management.
///
/// Built with `ProcessGroup::try_from(pid)` from the pid of a child that leads
/// its own group (spawned with `process_group(0)` or `setsid`). The
/// conversion refuses every identifier for which `kill(-pgid, ...)` would
/// reach anything other than one job's group.
#[derive(Debug, Clone)]
pub struct ProcessGroup {
    /// Process group ID (same as the leader process PID)
    pgid: i32,
}

impl TryFrom<u32> for ProcessGroup {
    type Error = ExecutorError;

    fn try_from(pid: u32) -> Result<Self, Self::Error> {
        let pgid = i32::try_from(pid)
            .ok()
            .filter(|_| pid >= LOWEST_GROUP)
            .filter(|pgid| *pgid != getpgrp().as_raw())
            .ok_or(ExecutorError::InvalidProcessGroup(pid))?;
        Ok(Self { pgid })
    }
}

impl ProcessGroup {
    /// Get the process group ID
    pub fn pgid(&self) -> i32 {
        self.pgid
    }

    /// Send SIGTERM to the entire process group.
    ///
    /// This will attempt to gracefully terminate all processes in the group.
    pub fn terminate(&self) -> Result<(), ExecutorError> {
        self.signal(Signal::SIGTERM)
    }

    /// Send SIGKILL to the entire process group.
    ///
    /// This forcefully kills all processes in the group.
    pub fn kill(&self) -> Result<(), ExecutorError> {
        self.signal(Signal::SIGKILL)
    }

    /// Perform a graceful shutdown: SIGTERM, wait, then SIGKILL if needed.
    ///
    /// This first sends SIGTERM and waits for the grace period, then sends
    /// SIGKILL if processes are still running.
    pub async fn graceful_kill(&self, grace_period: Duration) -> Result<(), ExecutorError> {
        self.terminate()?;

        sleep(grace_period).await;

        let _ = self.kill();

        Ok(())
    }

    /// Check if the process group is still running.
    ///
    /// Returns true if any process in the group is still alive.
    pub fn is_alive(&self) -> bool {
        kill(Pid::from_raw(-self.pgid), None).is_ok()
    }

    fn signal(&self, signal: Signal) -> Result<(), ExecutorError> {
        match kill(Pid::from_raw(-self.pgid), signal) {
            Ok(()) | Err(Errno::ESRCH) => Ok(()),
            Err(error) => Err(ExecutorError::ProcessGroupFailed(format!(
                "Failed to send {signal} to process group {}: {error}",
                self.pgid
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::executor::sleeper::Sleeper;

    use super::*;

    #[test]
    fn a_pid_that_is_not_one_jobs_group_is_refused() {
        let own = u32::try_from(getpgrp().as_raw()).unwrap();
        for pid in [0, 1, own, i32::MAX as u32 + 1, u32::MAX] {
            assert!(
                matches!(
                    ProcessGroup::try_from(pid),
                    Err(ExecutorError::InvalidProcessGroup(refused)) if refused == pid
                ),
                "{pid} must not become a process group"
            );
        }
    }

    #[test]
    fn a_child_leading_its_own_group_is_accepted() {
        let sleeper = Sleeper::start();

        let group = ProcessGroup::try_from(sleeper.pid()).unwrap();

        assert_eq!(group.pgid(), i32::try_from(sleeper.pid()).unwrap());
    }

    #[test]
    fn terminate_signals_the_group_with_sigterm() {
        let mut sleeper = Sleeper::start();

        sleeper.group().terminate().unwrap();

        assert_eq!(sleeper.wait(), Some(Signal::SIGTERM as i32));
    }

    #[test]
    fn kill_signals_the_group_with_sigkill() {
        let mut sleeper = Sleeper::start();

        sleeper.group().kill().unwrap();

        assert_eq!(sleeper.wait(), Some(Signal::SIGKILL as i32));
    }

    #[test]
    fn a_group_is_alive_until_its_last_process_is_reaped() {
        let mut sleeper = Sleeper::start();
        let group = sleeper.group();
        assert!(group.is_alive());

        group.kill().unwrap();
        sleeper.wait();

        assert!(!group.is_alive());
    }

    #[test]
    fn signalling_a_group_that_has_gone_is_not_an_error() {
        let mut sleeper = Sleeper::start();
        let group = sleeper.group();
        group.kill().unwrap();
        sleeper.wait();

        assert!(group.terminate().is_ok());
        assert!(group.kill().is_ok());
    }
}
