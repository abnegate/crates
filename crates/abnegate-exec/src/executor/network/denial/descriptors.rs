use std::io;

use nix::libc;

/// The lowest descriptor above standard input, output and error.
const FIRST_INHERITED: libc::c_int = 3;

/// Mark every descriptor above standard error close-on-exec, so a socket the
/// process inherited, which a denied command could still send through, never
/// reaches the program it executes.
///
/// It allocates nothing, so it may run between fork and exec. A descriptor
/// made inheritable after it runs is still inherited.
pub(super) fn close_on_exec() -> io::Result<()> {
    #[allow(unsafe_code)]
    let marked = unsafe {
        libc::syscall(
            libc::SYS_close_range,
            FIRST_INHERITED.unsigned_abs(),
            libc::c_uint::MAX,
            libc::CLOSE_RANGE_CLOEXEC,
        )
    };
    if marked == 0 {
        return Ok(());
    }
    for descriptor in FIRST_INHERITED..open_limit()? {
        // A descriptor that is not open answers `EBADF`, which leaves
        // nothing to mark.
        #[allow(unsafe_code)]
        unsafe {
            libc::fcntl(descriptor, libc::F_SETFD, libc::FD_CLOEXEC);
        }
    }
    Ok(())
}

/// One past the highest descriptor the process may open.
fn open_limit() -> io::Result<libc::c_int> {
    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    #[allow(unsafe_code)]
    let read = unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &raw mut limit) };
    if read == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(libc::c_int::try_from(limit.rlim_cur).unwrap_or(libc::c_int::MAX))
}
