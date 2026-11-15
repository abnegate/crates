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
/// place fails the spawn, and a host that cannot keep the command from
/// tracing out of it fails the spawn before its child does anything else.
pub(super) fn deny(command: &mut Command) {
    match Ptrace::contain() {
        Ptrace::Refused(errno) => refuse(command, errno),
        ptrace => confine(command, ptrace),
    }
}

/// Put the denial in place on `command`, held from tracing out by `ptrace`.
fn confine(command: &mut Command, ptrace: Ptrace) {
    let filter = Filter::new();
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

/// Fail `command`'s spawn with `errno` as soon as its child starts.
fn refuse(command: &mut Command, errno: libc::c_int) {
    // Runs between fork and exec, and only builds an error from its number,
    // which allocates nothing.
    #[allow(unsafe_code)]
    unsafe {
        command.pre_exec(move || Err(io::Error::from_raw_os_error(errno)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_refused_command_fails_to_spawn_with_its_error_number() {
        let mut command = Command::new("true");
        refuse(&mut command, libc::EPERM);

        let error = command
            .status()
            .await
            .expect_err("a refused command fails to spawn");

        assert_eq!(error.raw_os_error(), Some(libc::EPERM));
    }
}
