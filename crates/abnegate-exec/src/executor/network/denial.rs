mod landlock;
mod ptrace;

use std::io;

use nix::libc;
use tokio::process::Command;

use super::descriptors;
use super::filter::Filter;
use ptrace::Ptrace;

/// Keep `command`'s process, and everything it starts, off the network from
/// just before it executes: it inherits no descriptor beyond its standard
/// streams, gains no privilege by executing a program, can trace no process
/// outside it, and runs under a [`Filter`]. A step that cannot be put in
/// place fails the spawn.
pub(super) fn deny(command: &mut Command) {
    let filter = Filter::new();
    let ptrace = Ptrace::contain();
    // Runs between fork and exec, so it allocates nothing: the filter and the
    // Landlock ruleset were built before the fork, and every step is a bare
    // syscall wrapper.
    #[allow(unsafe_code)]
    unsafe {
        command.pre_exec(move || {
            descriptors::close_on_exec()?;
            if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) == -1 {
                return Err(io::Error::last_os_error());
            }
            ptrace.confine()?;
            filter.install()
        });
    }
}
