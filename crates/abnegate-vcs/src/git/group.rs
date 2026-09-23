use nix::errno::Errno;
use nix::sys::signal::Signal;
use nix::sys::signal::killpg;
use nix::unistd::Pid;

/// The process group a git command and every helper it starts run in, killed
/// when the command is abandoned -- timed out, or its future dropped -- and
/// left alone once it has been waited for, since its identifier is then free
/// for the system to hand to another group.
///
/// The group remains separate from the server so cancellation cannot signal
/// another task. Drop covers timeout and future cancellation, not the server
/// being killed; abrupt process death requires the hosting supervisor to tear
/// down its group.
pub(super) struct Group {
    id: Pid,
    armed: bool,
}

impl Group {
    pub(super) fn new(id: Pid) -> Self {
        Self { id, armed: true }
    }

    /// The command was waited for, so nothing is left to tear down.
    pub(super) fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for Group {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        if let Err(error) = killpg(self.id, Signal::SIGKILL)
            && error != Errno::ESRCH
        {
            tracing::warn!(%error, group = self.id.as_raw(), "Could not terminate Git process group");
        }
    }
}
