use std::fs;
use std::fs::DirEntry;
use std::io;
use std::os::fd::AsRawFd;
use std::os::fd::FromRawFd;
use std::os::fd::OwnedFd;
use std::os::fd::RawFd;
use std::os::unix::fs::MetadataExt;

use nix::libc;
use nix::sys::signal::Signal;
use nix::sys::signal::kill;
use nix::unistd::Pid;
use nix::unistd::Uid;
use nix::unistd::geteuid;

const PROCESSES: &str = "/proc";

/// Reads `/proc` for the processes that carry a marker's entry.
pub(super) struct Scanner {
    own: u32,
    user: Uid,
}

impl Scanner {
    pub(super) fn new() -> io::Result<Self> {
        Ok(Self {
            own: std::process::id(),
            user: geteuid(),
        })
    }

    /// Processes of the caller's effective user, other than the caller, whose
    /// own environment holds `entry` exactly.
    pub(super) fn find(&mut self, entry: &[u8]) -> io::Result<Vec<u32>> {
        let processes = fs::read_dir(PROCESSES)?
            .filter_map(Result::ok)
            .filter_map(|process| {
                let pid = pid(&process)?;
                let matched =
                    pid != self.own && owned_by(&process, self.user) && carries(pid, entry);
                matched.then_some(pid)
            })
            .collect();
        Ok(processes)
    }

    /// Send `signal` to `pid` if it still carries `entry`, checked while
    /// holding a pidfd where one is available, so the signal cannot reach a
    /// process that took the pid over after the check. Returns whether it was
    /// delivered.
    pub(super) fn deliver(&mut self, pid: u32, entry: &[u8], signal: Signal) -> io::Result<bool> {
        let Ok(target) = libc::pid_t::try_from(pid) else {
            return Ok(false);
        };
        let delivered = match open(target) {
            Ok(descriptor) => {
                if_carries(pid, entry, || send(&descriptor, signal)).or_else(|error| {
                    if without_pidfd(&error, Call::Send) {
                        fallback(target, entry, signal)
                    } else {
                        Err(error)
                    }
                })
            }
            Err(error) if error.raw_os_error() == Some(libc::ESRCH) => Ok(false),
            Err(error) if without_pidfd(&error, Call::Open) => fallback(target, entry, signal),
            Err(error) => Err(error),
        };
        match delivered {
            Err(error) if error.raw_os_error() == Some(libc::ESRCH) => Ok(false),
            delivered => delivered,
        }
    }
}

/// The pidfd call a sweep makes.
#[derive(Clone, Copy)]
enum Call {
    Open,
    Send,
}

/// Whether `call` failed only because the host offers no pidfd, which leaves
/// the pid-based [`fallback`] to signal the process: the kernel predates
/// pidfds, or, for `pidfd_open`, a seccomp filter refuses them. Any other
/// failure is the sweep's to report.
fn without_pidfd(error: &io::Error, call: Call) -> bool {
    match error.raw_os_error() {
        Some(libc::ENOSYS) => true,
        Some(libc::EPERM) => matches!(call, Call::Open),
        _ => false,
    }
}

fn pid(process: &DirEntry) -> Option<u32> {
    process.file_name().to_str()?.parse().ok()
}

fn owned_by(process: &DirEntry, user: Uid) -> bool {
    process
        .metadata()
        .is_ok_and(|metadata| metadata.uid() == user.as_raw())
}

/// Whether the environment of `pid` holds `entry`. A zombie's environment
/// reads empty, so a zombie never matches.
fn carries(pid: u32, entry: &[u8]) -> bool {
    fs::read(format!("{PROCESSES}/{pid}/environ")).is_ok_and(|environment| {
        environment
            .split(|&byte| byte == 0)
            .any(|variable| variable == entry)
    })
}

/// Signal `pid` if it carries `entry` without a pidfd, which leaves the pid
/// free to be reused between the check and the signal.
fn fallback(pid: libc::pid_t, entry: &[u8], signal: Signal) -> io::Result<bool> {
    if_carries(pid.cast_unsigned(), entry, || {
        kill(Pid::from_raw(pid), signal).map_err(io::Error::from)
    })
}

fn if_carries(pid: u32, entry: &[u8], sender: impl FnOnce() -> io::Result<()>) -> io::Result<bool> {
    if !carries(pid, entry) {
        return Ok(false);
    }
    sender()?;
    Ok(true)
}

fn open(pid: libc::pid_t) -> io::Result<OwnedFd> {
    // SAFETY: pidfd_open takes no pointers.
    let descriptor = checked(unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) })?;
    let descriptor = RawFd::try_from(descriptor).map_err(io::Error::other)?;
    // SAFETY: pidfd_open returned a new descriptor, which nothing else owns.
    Ok(unsafe { OwnedFd::from_raw_fd(descriptor) })
}

fn send(descriptor: &OwnedFd, signal: Signal) -> io::Result<()> {
    // SAFETY: the pidfd stays open for the call, and a null siginfo has the
    // kernel fill in what kill would.
    checked(unsafe {
        libc::syscall(
            libc::SYS_pidfd_send_signal,
            descriptor.as_raw_fd(),
            signal as libc::c_int,
            std::ptr::null::<libc::siginfo_t>(),
            0,
        )
    })?;
    Ok(())
}

fn checked(result: libc::c_long) -> io::Result<libc::c_long> {
    if result == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::super::Marker;
    use super::super::straggler::Behaviour;
    use super::super::straggler::Straggler;
    use super::super::straggler::VARIABLE;
    use super::super::straggler::exits_within;
    use super::*;

    #[test]
    fn only_a_host_without_pidfds_falls_back_to_the_pid() {
        let failure = |errno| io::Error::from_raw_os_error(errno);

        assert!(without_pidfd(&failure(libc::ENOSYS), Call::Open));
        assert!(without_pidfd(&failure(libc::ENOSYS), Call::Send));
        assert!(without_pidfd(&failure(libc::EPERM), Call::Open));
        assert!(
            !without_pidfd(&failure(libc::EPERM), Call::Send),
            "a process that refused its pidfd's signal was signalled by pid"
        );
        for errno in [libc::EMFILE, libc::ENFILE, libc::ENOMEM, libc::EINVAL] {
            assert!(
                !without_pidfd(&failure(errno), Call::Open),
                "pidfd_open failing with {errno} fell back to the pid"
            );
        }
    }

    #[test]
    fn fallback_kills_only_a_marked_process() {
        let marker = Marker::new(VARIABLE).unwrap();
        let entry = marker.entry();
        let marked = Straggler::start(&mut Straggler::marked(Behaviour::Escapes, &marker), 1);
        let mut unmarked = Straggler::start(&mut Straggler::command(Behaviour::Escapes), 1);

        for pid in [marked.pid(), unmarked.pid()] {
            fallback(pid.cast_signed(), entry.as_bytes(), Signal::SIGKILL).unwrap();
        }

        assert!(
            exits_within(marked.pid()),
            "marked process {} outlived SIGKILL",
            marked.pid()
        );
        assert!(
            unmarked.is_running(),
            "unmarked process {} was killed",
            unmarked.pid()
        );
    }

    #[test]
    fn deliver_kills_only_a_marked_process() {
        let marker = Marker::new(VARIABLE).unwrap();
        let entry = marker.entry();
        let marked = Straggler::start(&mut Straggler::marked(Behaviour::Escapes, &marker), 1);
        let mut unmarked = Straggler::start(&mut Straggler::command(Behaviour::Escapes), 1);

        let mut scanner = Scanner::new().unwrap();
        let delivered = scanner
            .deliver(marked.pid(), entry.as_bytes(), Signal::SIGKILL)
            .unwrap();
        let spared = !scanner
            .deliver(unmarked.pid(), entry.as_bytes(), Signal::SIGKILL)
            .unwrap();

        assert!(delivered && exits_within(marked.pid()));
        assert!(spared && unmarked.is_running());
    }

    #[test]
    fn a_process_that_has_gone_takes_no_signal() {
        let marker = Marker::new(VARIABLE).unwrap();
        let entry = marker.entry();
        let mut straggler = Straggler::start(&mut Straggler::marked(Behaviour::Stays, &marker), 1);
        let pid = straggler.pid();
        straggler.kill();
        assert!(exits_within(pid));

        let mut scanner = Scanner::new().unwrap();
        assert!(
            !scanner
                .deliver(pid, entry.as_bytes(), Signal::SIGKILL)
                .unwrap()
        );
        assert!(scanner.find(entry.as_bytes()).unwrap().is_empty());
    }
}
