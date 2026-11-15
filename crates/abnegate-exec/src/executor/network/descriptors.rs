#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;

use nix::libc;

#[cfg(target_os = "linux")]
pub(super) use linux::close_on_exec;
#[cfg(target_os = "macos")]
pub(super) use macos::close_on_exec;

/// The lowest descriptor above standard input, output and error.
const FIRST_INHERITED: libc::c_int = 3;

/// Mark `descriptor` close-on-exec. One that is not open answers `EBADF`,
/// which leaves nothing to mark.
///
/// It allocates nothing, so it may run between fork and exec.
fn mark(descriptor: libc::c_int) {
    #[allow(unsafe_code)]
    unsafe {
        libc::fcntl(descriptor, libc::F_SETFD, libc::FD_CLOEXEC);
    }
}

/// Whether a descriptor the test process holds without close-on-exec is
/// still open in a program executed after `hook` runs in its child.
#[cfg(test)]
fn inherited_through(hook: fn() -> std::io::Result<()>) -> bool {
    use std::os::fd::AsRawFd;
    use std::os::fd::FromRawFd;
    use std::os::fd::OwnedFd;
    use std::os::unix::process::CommandExt;

    let file = std::fs::File::open("/dev/null").expect("/dev/null");
    #[allow(unsafe_code)]
    let duplicate = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_DUPFD, 200) };
    assert_ne!(
        duplicate,
        -1,
        "an inheritable duplicate: {}",
        std::io::Error::last_os_error()
    );
    // The descriptor was just made, and nothing else owns it.
    #[allow(unsafe_code)]
    let duplicate = unsafe { OwnedFd::from_raw_fd(duplicate) };
    let mut command = std::process::Command::new("test");
    command.args(["-e", &format!("/dev/fd/{}", duplicate.as_raw_fd())]);
    #[allow(unsafe_code)]
    unsafe {
        command.pre_exec(hook);
    }
    command.status().expect("test's status").success()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_descriptor_left_alone_is_inherited() {
        assert!(inherited_through(|| Ok(())));
    }

    #[test]
    fn no_descriptor_beyond_the_standard_streams_is_inherited() {
        assert!(!inherited_through(close_on_exec));
    }
}
