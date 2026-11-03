use std::io;
use std::os::unix::process::ExitStatusExt;
use std::process::ExitStatus;

use nix::errno::Errno;
use nix::libc::timespec;
use nix::sys::event::EvFlags;
use nix::sys::event::EventFilter;
use nix::sys::event::FilterFlag;
use nix::sys::event::KEvent;
use nix::sys::event::Kqueue;
use nix::unistd::Pid;

const IMMEDIATELY: timespec = timespec {
    tv_sec: 0,
    tv_nsec: 0,
};

/// Checks whether a child has exited, leaving it unreaped, and keeps the
/// status it exited with.
pub(super) struct Probe {
    pid: Pid,
    queue: Kqueue,
    /// Whether the queue watches the child. A child already exiting cannot
    /// be registered, and is asked for its status with `waitid` instead.
    registered: bool,
    status: Option<ExitStatus>,
}

impl Probe {
    pub(super) fn watch(pid: Pid) -> io::Result<Self> {
        let queue = Kqueue::new()?;
        let registration = KEvent::new(
            usize::try_from(pid.as_raw()).map_err(io::Error::other)?,
            EventFilter::EVFILT_PROC,
            EvFlags::EV_ADD | EvFlags::EV_ONESHOT,
            exit_filter(),
            0,
            0,
        );
        let registered = match queue.kevent(&[registration], &mut [], None) {
            Ok(_) => true,
            Err(Errno::ESRCH) => false,
            Err(errno) => return Err(errno.into()),
        };
        Ok(Self {
            pid,
            queue,
            registered,
            status: None,
        })
    }

    /// The status the child exited with, or `None` while it runs.
    pub(super) fn exit(&mut self) -> io::Result<Option<ExitStatus>> {
        if self.status.is_none() {
            self.status = if self.registered {
                self.event()?
            } else {
                exiting(self.pid)?
            };
        }
        Ok(self.status)
    }

    /// The status the queue's exit event carries, once it has one.
    fn event(&mut self) -> io::Result<Option<ExitStatus>> {
        let mut events = [KEvent::new(
            0,
            EventFilter::EVFILT_PROC,
            EvFlags::empty(),
            FilterFlag::empty(),
            0,
            0,
        )];
        if self.queue.kevent(&[], &mut events, Some(IMMEDIATELY))? == 0 {
            return Ok(None);
        }
        let status = i32::try_from(events[0].data()).map_err(io::Error::other)?;
        Ok(Some(ExitStatus::from_raw(status)))
    }
}

/// `NOTE_EXIT`, asking for the exit status in the event's data, which macOS
/// puts there only on `NOTE_EXITSTATUS`.
#[cfg(target_vendor = "apple")]
fn exit_filter() -> FilterFlag {
    FilterFlag::NOTE_EXIT | FilterFlag::NOTE_EXITSTATUS
}

/// `NOTE_EXIT`, whose event FreeBSD always gives the exit status in its data.
#[cfg(target_os = "freebsd")]
fn exit_filter() -> FilterFlag {
    FilterFlag::NOTE_EXIT
}

/// The status of a child that is exiting, read without reaping it, or `None`
/// until it can be waited on: macOS refuses to watch a child part way
/// through its exit, a moment before `waitid` sees it.
#[cfg(target_os = "freebsd")]
fn exiting(pid: Pid) -> io::Result<Option<ExitStatus>> {
    use nix::sys::wait::Id;
    use nix::sys::wait::WaitPidFlag;
    use nix::sys::wait::waitid;

    Ok(super::reported(waitid(
        Id::Pid(pid),
        WaitPidFlag::WEXITED | WaitPidFlag::WNOHANG | WaitPidFlag::WNOWAIT,
    )?))
}

/// The status of a child that is exiting, read without reaping it, or `None`
/// until it can be waited on: macOS refuses to watch a child part way
/// through its exit, a moment before `waitid` sees it. nix has no `waitid`
/// for Apple hosts.
#[cfg(target_vendor = "apple")]
#[allow(unsafe_code)]
fn exiting(pid: Pid) -> io::Result<Option<ExitStatus>> {
    use nix::libc;

    let id = libc::id_t::try_from(pid.as_raw()).map_err(io::Error::other)?;
    // SAFETY: siginfo_t holds only integers and a pointer, all valid zeroed.
    let mut information: libc::siginfo_t = unsafe { std::mem::zeroed() };
    // SAFETY: `information` is valid for waitid to write one siginfo_t.
    let result = unsafe {
        libc::waitid(
            libc::P_PID,
            id,
            &raw mut information,
            libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
        )
    };
    if result == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(match information.si_code {
        libc::CLD_EXITED => Some(super::exited(information.si_status)),
        libc::CLD_KILLED => Some(super::signalled(information.si_status, false)),
        libc::CLD_DUMPED => Some(super::signalled(information.si_status, true)),
        _ => None,
    })
}
