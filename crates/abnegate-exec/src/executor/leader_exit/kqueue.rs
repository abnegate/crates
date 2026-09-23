use std::io;

use nix::errno::Errno;
use nix::libc::timespec;
use nix::sys::event::EvFlags;
use nix::sys::event::EventFilter;
use nix::sys::event::FilterFlag;
use nix::sys::event::KEvent;
use nix::sys::event::Kqueue;
use nix::unistd::Pid;
use tokio::signal::unix::Signal;
use tokio::signal::unix::SignalKind;
use tokio::signal::unix::signal;

const IMMEDIATELY: timespec = timespec {
    tv_sec: 0,
    tv_nsec: 0,
};

/// Resolves once a child has exited, leaving it unreaped.
pub(in crate::executor) struct LeaderExit {
    queue: Kqueue,
    signals: Signal,
    exited: bool,
}

impl LeaderExit {
    pub(in crate::executor) fn watch(pid: Pid) -> io::Result<Self> {
        let signals = signal(SignalKind::child())?;
        let queue = Kqueue::new()?;
        let registration = KEvent::new(
            usize::try_from(pid.as_raw()).map_err(io::Error::other)?,
            EventFilter::EVFILT_PROC,
            EvFlags::EV_ADD | EvFlags::EV_ONESHOT,
            FilterFlag::NOTE_EXIT,
            0,
            0,
        );
        let exited = match queue.kevent(&[registration], &mut [], None) {
            Ok(_) => false,
            Err(Errno::ESRCH) => true,
            Err(errno) => return Err(errno.into()),
        };
        Ok(Self {
            queue,
            signals,
            exited,
        })
    }

    pub(in crate::executor) async fn wait(&mut self) -> io::Result<()> {
        while !self.has_exited()? {
            if self.signals.recv().await.is_none() {
                return Err(io::Error::other("the SIGCHLD stream ended"));
            }
        }
        Ok(())
    }

    fn has_exited(&mut self) -> io::Result<bool> {
        if !self.exited {
            let mut events = [KEvent::new(
                0,
                EventFilter::EVFILT_PROC,
                EvFlags::empty(),
                FilterFlag::empty(),
                0,
                0,
            )];
            self.exited = self.queue.kevent(&[], &mut events, Some(IMMEDIATELY))? > 0;
        }
        Ok(self.exited)
    }
}
