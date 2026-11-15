mod landlock;

use std::io;
use std::os::fd::AsFd;
use std::os::fd::OwnedFd;

use nix::libc;
use tokio::process::Command;

use super::descriptors;
use super::filter::Filter;

/// Keep `command`'s process, and everything it starts, off the network from
/// just before it executes: it inherits no descriptor beyond its standard
/// streams, gains no privilege by executing a program, can trace no process
/// outside it, and runs under a [`Filter`]. A step that cannot be put in
/// place fails the spawn, and a host without Landlock, which alone keeps
/// the command from tracing out of it, fails the spawn before its child does
/// anything else.
pub(super) fn deny(command: &mut Command) {
    match landlock::ruleset() {
        Ok(ruleset) => confine(command, ruleset),
        Err(error) => refuse(command, landlock::refusal(&error)),
    }
}

/// Put the denial in place on `command`, kept from tracing out of it by the
/// domain `ruleset` makes.
fn confine(command: &mut Command, ruleset: OwnedFd) {
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
            landlock::restrict(ruleset.as_fd())?;
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

    #[tokio::test]
    async fn a_host_without_landlock_fails_to_spawn_a_denied_command() {
        let Err(unavailable) = landlock::ruleset() else {
            return;
        };
        let mut command = Command::new("true");
        deny(&mut command);

        let error = command
            .status()
            .await
            .expect_err("a denied command fails to spawn without Landlock");

        assert_eq!(error.raw_os_error(), Some(landlock::refusal(&unavailable)));
    }
}
