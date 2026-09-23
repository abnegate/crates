use std::os::fd::OwnedFd;

use nix::fcntl::FcntlArg;
use nix::fcntl::FdFlag;
use nix::fcntl::fcntl;
use nix::unistd::setsid;
use tokio::process::Command;

/// Make `command` lead a new session, so its whole process tree can be
/// signalled as one group, and let it inherit `descriptor`, which is
/// otherwise closed on exec like every descriptor this crate opens.
pub(crate) fn lead(command: &mut Command, descriptor: Option<OwnedFd>) {
    // Runs between fork and exec, where only async-signal-safe calls are
    // permitted: `setsid` and `fcntl` are, and neither they nor the
    // conversion of their errors allocates.
    #[allow(unsafe_code)]
    unsafe {
        command.pre_exec(move || {
            setsid()?;
            if let Some(descriptor) = &descriptor {
                fcntl(descriptor, FcntlArg::F_SETFD(FdFlag::empty()))?;
            }
            Ok(())
        });
    }
}
