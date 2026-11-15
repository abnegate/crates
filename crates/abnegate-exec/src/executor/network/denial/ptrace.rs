use std::fs;
use std::io;
use std::os::fd::AsFd;
use std::os::fd::OwnedFd;

use nix::libc;

use super::landlock;

/// Where Yama publishes how far a process may trace another.
const YAMA_SCOPE: &str = "/proc/sys/kernel/yama/ptrace_scope";

/// Where the executor's own capabilities are published.
const STATUS: &str = "/proc/self/status";

/// The line of [`STATUS`] that holds the permitted capabilities, in
/// hexadecimal, which bound everything a child without new privileges holds.
const PERMITTED: &str = "CapPrm:";

/// `CAP_SYS_PTRACE`, which lets a process trace past Yama's restricted and
/// administrator-only scopes.
const SYS_PTRACE: u32 = 19;

/// How a denied command is kept from tracing, or writing the memory of
/// through `/proc`, a process outside it, the executor that reaches the
/// network above all.
pub(super) enum Ptrace {
    /// A Landlock domain, which no process in it can trace out of.
    Landlock(OwnedFd),
    /// Yama already keeps a process from tracing anything but its own
    /// descendants, and the command holds no capability that lifts it.
    Yama,
    /// Nothing does, so the command must not run: its spawn fails with this
    /// error number.
    Refused(libc::c_int),
}

impl Ptrace {
    /// How this host keeps a command about to start from tracing out of it,
    /// read before the fork.
    pub(super) fn contain() -> Self {
        match landlock::ruleset() {
            Ok(ruleset) => Self::Landlock(ruleset),
            Err(error) if landlock::unavailable(&error) => {
                if yama_contains(yama_scope(), may_trace_anything()) {
                    Self::Yama
                } else {
                    Self::Refused(libc::EPERM)
                }
            }
            Err(error) => Self::Refused(error.raw_os_error().unwrap_or(libc::EPERM)),
        }
    }

    /// Put the containment in place on the calling thread, which must have
    /// set `no_new_privs` first.
    ///
    /// It allocates nothing, so it may run between fork and exec.
    pub(super) fn confine(&self) -> io::Result<()> {
        match self {
            Self::Landlock(ruleset) => landlock::restrict(ruleset.as_fd()),
            Self::Yama => Ok(()),
            Self::Refused(errno) => Err(io::Error::from_raw_os_error(*errno)),
        }
    }
}

/// Whether Yama, at `scope`, keeps a process from tracing anything but its
/// own descendants: from the restricted scope 1 up, unless the process is
/// `capable` of tracing past it, and at the scope 3 that allows no tracing at
/// all, even then.
fn yama_contains(scope: Option<u8>, capable: bool) -> bool {
    match scope {
        Some(3..) => true,
        Some(1 | 2) => !capable,
        Some(0) | None => false,
    }
}

/// Yama's scope, or nothing on a kernel without Yama.
fn yama_scope() -> Option<u8> {
    fs::read_to_string(YAMA_SCOPE).ok()?.trim().parse().ok()
}

/// Whether the executor holds `CAP_SYS_PTRACE` among its permitted
/// capabilities, which a child could raise. Status that cannot be read
/// counts as holding it.
fn may_trace_anything() -> bool {
    let permitted = fs::read_to_string(STATUS).ok().and_then(|status| {
        status
            .lines()
            .find_map(|line| line.strip_prefix(PERMITTED))
            .and_then(|capabilities| u64::from_str_radix(capabilities.trim(), 16).ok())
    });
    !matches!(permitted, Some(capabilities) if capabilities & (1 << SYS_PTRACE) == 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_host_with_landlock_contains_a_command_with_it() {
        if landlock::ruleset().is_err() {
            return;
        }

        assert!(matches!(Ptrace::contain(), Ptrace::Landlock(_)));
    }

    #[test]
    fn yama_contains_a_command_from_the_restricted_scope_up() {
        for scope in [1, 2, 3] {
            assert!(yama_contains(Some(scope), false), "{scope}");
        }
    }

    #[test]
    fn yama_does_not_contain_a_command_at_the_classic_scope_or_without_yama() {
        for capable in [false, true] {
            assert!(!yama_contains(Some(0), capable));
            assert!(!yama_contains(None, capable));
        }
    }

    #[test]
    fn yama_does_not_contain_a_command_that_may_trace_past_it() {
        assert!(!yama_contains(Some(1), true));
        assert!(!yama_contains(Some(2), true));
        assert!(yama_contains(Some(3), true));
    }

    #[test]
    fn a_refused_containment_fails_with_its_error_number() {
        let error = Ptrace::Refused(libc::EPERM)
            .confine()
            .expect_err("a refused containment fails");

        assert_eq!(error.raw_os_error(), Some(libc::EPERM));
    }
}
