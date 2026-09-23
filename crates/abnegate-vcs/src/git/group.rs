/// The group remains separate from the server so cancellation cannot signal
/// another task. Drop covers timeout/future cancellation, not server SIGKILL;
/// abrupt process death requires the hosting supervisor to tear down its group.
#[cfg(unix)]
pub(super) struct Group(pub(super) nix::unistd::Pid);

#[cfg(unix)]
impl Drop for Group {
    fn drop(&mut self) {
        if let Err(error) = nix::sys::signal::killpg(self.0, nix::sys::signal::Signal::SIGKILL)
            && error != nix::errno::Errno::ESRCH
        {
            tracing::warn!(%error, group = self.0.as_raw(), "Could not terminate Git process group");
        }
    }
}
