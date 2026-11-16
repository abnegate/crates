use std::io;
use std::os::fd::AsRawFd;
use std::os::fd::BorrowedFd;
use std::os::fd::FromRawFd;
use std::os::fd::OwnedFd;

use nix::fcntl::OFlag;
use nix::fcntl::open;
use nix::libc;
use nix::sys::stat::Mode;

/// `LANDLOCK_ACCESS_FS_MAKE_BLOCK`: creating a block device, the access the
/// ruleset handles, and allows everywhere.
const MAKE_BLOCK: u64 = 1 << 11;

/// `LANDLOCK_RULE_PATH_BENEATH`: a rule that allows access beneath a
/// directory.
const PATH_BENEATH: libc::c_int = 1;

/// `struct landlock_ruleset_attr` up to `handled_access_fs`, the prefix every
/// Landlock ABI accepts.
#[repr(C)]
struct RulesetAttributes {
    handled: u64,
}

/// `struct landlock_path_beneath_attr`.
#[repr(C, packed)]
struct PathBeneath {
    allowed: u64,
    parent: libc::c_int,
}

/// A Landlock ruleset that handles creating a block device and allows it
/// beneath `/`, so the domain it makes refuses no file access. What the
/// domain adds is Landlock's own scoping: a process in it can trace no
/// process outside it, nor reach that process's memory, environment or
/// descriptors through `/proc`, whatever mode the access asks for.
///
/// The ruleset closes on exec, as every Landlock ruleset does.
pub(super) fn ruleset() -> io::Result<OwnedFd> {
    let attributes = RulesetAttributes {
        handled: MAKE_BLOCK,
    };
    #[allow(unsafe_code)]
    let created = unsafe {
        libc::syscall(
            libc::SYS_landlock_create_ruleset,
            &raw const attributes,
            size_of::<RulesetAttributes>(),
            0 as libc::c_uint,
        )
    };
    if created == -1 {
        return Err(io::Error::last_os_error());
    }
    let created =
        libc::c_int::try_from(created).map_err(|_| io::Error::from(io::ErrorKind::InvalidData))?;
    // A ruleset just created is open, and nothing else owns it.
    #[allow(unsafe_code)]
    let ruleset = unsafe { OwnedFd::from_raw_fd(created) };
    let root = open(
        "/",
        OFlag::O_PATH | OFlag::O_DIRECTORY | OFlag::O_CLOEXEC,
        Mode::empty(),
    )?;
    let rule = PathBeneath {
        allowed: MAKE_BLOCK,
        parent: root.as_raw_fd(),
    };
    #[allow(unsafe_code)]
    let added = unsafe {
        libc::syscall(
            libc::SYS_landlock_add_rule,
            ruleset.as_raw_fd(),
            PATH_BENEATH,
            &raw const rule,
            0 as libc::c_uint,
        )
    };
    if added == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(ruleset)
}

/// The error number a denied command's spawn fails with when [`ruleset`]
/// fails with `error`: `EPERM` where this host has no Landlock to offer, as
/// on a kernel built without it, one booted with it off, or in a container
/// whose own seccomp profile refuses it, and the error's own number
/// otherwise.
pub(super) fn refusal(error: &io::Error) -> libc::c_int {
    match error.raw_os_error() {
        Some(libc::ENOSYS | libc::EOPNOTSUPP) | None => libc::EPERM,
        Some(errno) => errno,
    }
}

/// Put the calling thread, and everything it starts, in the domain
/// `ruleset` makes. The thread must have set `no_new_privs` first.
///
/// It allocates nothing, so it may run between fork and exec.
pub(super) fn restrict(ruleset: BorrowedFd<'_>) -> io::Result<()> {
    #[allow(unsafe_code)]
    let restricted = unsafe {
        libc::syscall(
            libc::SYS_landlock_restrict_self,
            ruleset.as_raw_fd(),
            0 as libc::c_uint,
        )
    };
    if restricted == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_host_without_landlock_refuses_with_eperm() {
        for errno in [libc::ENOSYS, libc::EOPNOTSUPP, libc::EPERM] {
            assert_eq!(
                refusal(&io::Error::from_raw_os_error(errno)),
                libc::EPERM,
                "{errno}"
            );
        }
        assert_eq!(
            refusal(&io::Error::from(io::ErrorKind::InvalidData)),
            libc::EPERM
        );
    }

    #[test]
    fn any_other_failure_refuses_with_its_own_error_number() {
        assert_eq!(
            refusal(&io::Error::from_raw_os_error(libc::EMFILE)),
            libc::EMFILE
        );
    }
}
