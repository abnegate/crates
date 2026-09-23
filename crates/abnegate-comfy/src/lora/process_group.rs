#[cfg(unix)]
use nix::errno::Errno;
#[cfg(unix)]
use nix::sys::signal::Signal;
#[cfg(unix)]
use nix::sys::signal::killpg;
#[cfg(unix)]
use nix::unistd::Pid;
use std::io;
use tokio::process::Child;
use tokio::process::Command;

/// The external trainer's process group: the shell its command runs in, and
/// everything that shell starts.
///
/// Killing only the shell leaves the trainer it started holding the GPU, so a
/// kill here reaches the whole group, and dropping the handle is a kill.
pub(super) struct ProcessGroup {
    leader: Option<u32>,
}

impl ProcessGroup {
    /// Spawns `command` as the leader of a new process group.
    pub(super) fn spawn(command: &mut Command) -> io::Result<(Child, Self)> {
        #[cfg(unix)]
        command.process_group(0);
        let child = command.spawn()?;
        let group = Self { leader: child.id() };
        Ok((child, group))
    }

    /// Kills every process still in the group. Later calls do nothing.
    pub(super) fn kill(&mut self) {
        if let Some(leader) = self.leader.take() {
            terminate(leader);
        }
    }
}

impl Drop for ProcessGroup {
    fn drop(&mut self) {
        self.kill();
    }
}

#[cfg(unix)]
fn terminate(leader: u32) {
    let Some(group) = i32::try_from(leader)
        .ok()
        .filter(|id| *id > 0)
        .map(Pid::from_raw)
    else {
        return;
    };
    match killpg(group, Signal::SIGKILL) {
        Ok(()) | Err(Errno::ESRCH) => {}
        Err(error) => {
            tracing::warn!(group = group.as_raw(), %error, "could not kill the trainer's process group");
        }
    }
}

#[cfg(not(unix))]
fn terminate(_leader: u32) {}
