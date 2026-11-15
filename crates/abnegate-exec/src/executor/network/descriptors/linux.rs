mod entries;

use std::io;

use nix::libc;

use super::FIRST_INHERITED;
use super::mark;
use entries::Entries;

/// The words one `getdents64` read of `/proc/self/fd` fills, aligned for
/// the records the kernel writes into them.
const READ_WORDS: usize = 512;

/// Mark every descriptor above standard error close-on-exec, so a socket the
/// process inherited, which a denied command could still send through, never
/// reaches the program it executes.
///
/// `close_range` marks them in one call. Where it is refused, as a kernel
/// older than 5.11 or a container's own seccomp profile does, each descriptor
/// `/proc/self/fd` lists is marked instead, and a process that cannot list
/// them fails with the error that stopped it.
///
/// It allocates nothing, so it may run between fork and exec. A descriptor
/// made inheritable after it runs is still inherited.
pub(in crate::executor::network) fn close_on_exec() -> io::Result<()> {
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
    mark_listed()
}

/// Mark each descriptor above standard error that `/proc/self/fd` lists.
fn mark_listed() -> io::Result<()> {
    #[allow(unsafe_code)]
    let directory = unsafe {
        libc::open(
            c"/proc/self/fd".as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
        )
    };
    if directory == -1 {
        return Err(io::Error::last_os_error());
    }
    let marked = mark_entries(directory);
    #[allow(unsafe_code)]
    unsafe {
        libc::close(directory);
    }
    marked
}

/// Mark each descriptor above standard error that `directory`, an open
/// `/proc/self/fd`, lists, except `directory` itself.
fn mark_entries(directory: libc::c_int) -> io::Result<()> {
    let mut words = [0u64; READ_WORDS];
    loop {
        #[allow(unsafe_code)]
        let read = unsafe {
            libc::syscall(
                libc::SYS_getdents64,
                directory,
                words.as_mut_ptr(),
                size_of_val(&words),
            )
        };
        let read = match read {
            -1 => return Err(io::Error::last_os_error()),
            0 => return Ok(()),
            read => usize::try_from(read)
                .ok()
                .filter(|read| *read <= size_of_val(&words))
                .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidData))?,
        };
        // `words` outlives the view, and the kernel wrote `read` bytes of it.
        #[allow(unsafe_code)]
        let buffer = unsafe { std::slice::from_raw_parts(words.as_ptr().cast::<u8>(), read) };
        for descriptor in Entries::new(buffer) {
            let descriptor = descriptor?;
            if descriptor >= FIRST_INHERITED && descriptor != directory {
                mark(descriptor);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::inherited_through;
    use super::*;

    /// A soft descriptor limit well below the descriptor
    /// [`inherited_through`] holds, which a walk of the limit would miss.
    const LOWERED_LIMIT: libc::rlim_t = 100;

    fn lower_the_limit_then_mark_listed() -> io::Result<()> {
        let mut limit = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        #[allow(unsafe_code)]
        unsafe {
            if libc::getrlimit(libc::RLIMIT_NOFILE, &raw mut limit) == -1 {
                return Err(io::Error::last_os_error());
            }
            limit.rlim_cur = LOWERED_LIMIT;
            if libc::setrlimit(libc::RLIMIT_NOFILE, &raw const limit) == -1 {
                return Err(io::Error::last_os_error());
            }
        }
        mark_listed()
    }

    #[test]
    fn a_listing_marks_every_descriptor_above_the_standard_streams() {
        assert!(!inherited_through(mark_listed));
    }

    #[test]
    fn a_listing_marks_a_descriptor_above_the_open_limit() {
        assert!(!inherited_through(lower_the_limit_then_mark_listed));
    }
}
