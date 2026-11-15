use std::io;

use nix::libc;

use super::FIRST_INHERITED;
use super::mark;

/// How many open descriptors one listing holds, more than nearly every
/// process has. A process with more has its whole descriptor table marked by
/// number instead.
const LISTED: usize = 1024;

/// Mark every descriptor above standard error close-on-exec, so a socket the
/// process inherited never reaches the program it executes: seatbelt checks
/// connecting and sending to an address, not writing to a socket that is
/// already connected.
///
/// It allocates nothing, so it may run between fork and exec. A descriptor
/// made inheritable after it runs is still inherited.
pub(in crate::executor::network) fn close_on_exec() -> io::Result<()> {
    mark_listed::<LISTED>();
    Ok(())
}

/// Mark each descriptor above standard error that a listing of at most
/// `CAPACITY` open descriptors names. A listing that fails, or that may have
/// left some out because it filled, marks the whole descriptor table instead.
fn mark_listed<const CAPACITY: usize>() {
    const {
        assert!(CAPACITY > 0 && CAPACITY * size_of::<libc::proc_fdinfo>() <= i32::MAX as usize);
    }
    let mut listing = [libc::proc_fdinfo {
        proc_fd: 0,
        proc_fdtype: 0,
    }; CAPACITY];
    #[allow(unsafe_code)]
    let listed = unsafe {
        libc::proc_pidinfo(
            libc::getpid(),
            libc::PROC_PIDLISTFDS,
            0,
            listing.as_mut_ptr().cast(),
            (CAPACITY * size_of::<libc::proc_fdinfo>()) as libc::c_int,
        )
    };
    let count = usize::try_from(listed).map_or(0, |bytes| bytes / size_of::<libc::proc_fdinfo>());
    if count == 0 || count >= CAPACITY {
        mark_table();
        return;
    }
    for entry in &listing[..count] {
        if entry.proc_fd >= FIRST_INHERITED {
            mark(entry.proc_fd);
        }
    }
}

/// Mark every descriptor above standard error, up to the size of the
/// descriptor table, by number.
fn mark_table() {
    #[allow(unsafe_code)]
    let size = unsafe { libc::getdtablesize() };
    for descriptor in FIRST_INHERITED..size {
        mark(descriptor);
    }
}

#[cfg(test)]
mod tests {
    use super::super::inherited_through;
    use super::*;

    #[test]
    fn a_listing_that_fills_marks_the_whole_table() {
        assert!(!inherited_through(|| {
            mark_listed::<1>();
            Ok(())
        }));
    }
}
