//! Learning that a child has exited, and the status it exited with, without
//! reaping it.
//!
//! An unreaped child is a zombie that keeps its pid, and so its process
//! group's id, from being reused: signalling the group stays safe until the
//! child is waited on. Each check leaves the child in place -- `waitid` with
//! `WNOWAIT` on Linux, a `NOTE_EXIT` kqueue event, whose data holds the exit
//! status, on macOS and FreeBSD -- and runs on every `SIGCHLD` and every
//! [`POLL_INTERVAL`], so an exit is noticed even when its `SIGCHLD` never
//! arrives, such as when a ptrace-based tool swallows it.

#[cfg(any(target_vendor = "apple", target_os = "freebsd"))]
mod kqueue;
#[cfg(any(target_os = "linux", target_os = "android"))]
mod waitid;

use std::io;
use std::os::unix::process::ExitStatusExt;
use std::process::ExitStatus;
use std::time::Duration;

#[cfg(any(target_os = "linux", target_os = "android", target_os = "freebsd"))]
use nix::sys::wait::WaitStatus;
use nix::unistd::Pid;
use tokio::signal::unix::Signal;
use tokio::signal::unix::SignalKind;
use tokio::signal::unix::signal;

#[cfg(any(target_vendor = "apple", target_os = "freebsd"))]
use kqueue::Probe;
#[cfg(any(target_os = "linux", target_os = "android"))]
use waitid::Probe;

/// How long a wait goes without checking on the child when no `SIGCHLD`
/// prompts it to.
const POLL_INTERVAL: Duration = Duration::from_millis(250);

/// The bit `wait` sets in a status beside the signal that killed a child
/// when the child dumped core.
const CORE_DUMPED: i32 = 0x80;

/// Resolves once a child has exited, leaving it unreaped.
pub(in crate::executor) struct LeaderExit {
    probe: Probe,
    signals: Signal,
}

impl LeaderExit {
    pub(in crate::executor) fn watch(pid: Pid) -> io::Result<Self> {
        Self::watch_on(pid, SignalKind::child())
    }

    /// Watch `pid`, checking on it whenever `kind` arrives and at every
    /// [`POLL_INTERVAL`].
    fn watch_on(pid: Pid, kind: SignalKind) -> io::Result<Self> {
        let signals = signal(kind)?;
        Ok(Self {
            probe: Probe::watch(pid)?,
            signals,
        })
    }

    /// The status the child exited with, read without reaping it.
    ///
    /// Cancel safe: dropping the future loses nothing, and the next wait
    /// checks on the child before it sleeps.
    pub(in crate::executor) async fn wait(&mut self) -> io::Result<ExitStatus> {
        loop {
            if let Some(status) = self.probe.exit()? {
                return Ok(status);
            }
            tokio::select! {
                received = self.signals.recv() => {
                    if received.is_none() {
                        return Err(io::Error::other("the SIGCHLD stream ended"));
                    }
                }
                () = tokio::time::sleep(POLL_INTERVAL) => {}
            }
        }
    }
}

/// The exit `status` a `waitid` reported, or `None` while the child runs.
#[cfg(any(target_os = "linux", target_os = "android", target_os = "freebsd"))]
fn reported(status: WaitStatus) -> Option<ExitStatus> {
    match status {
        WaitStatus::Exited(_, code) => Some(exited(code)),
        WaitStatus::Signaled(_, signal, dumped) => Some(signalled(signal as i32, dumped)),
        _ => None,
    }
}

/// The status `wait` reports for a child that exited with `code`.
fn exited(code: i32) -> ExitStatus {
    ExitStatus::from_raw((code & 0xff) << 8)
}

/// The status `wait` reports for a child `signal` killed, which `dumped`
/// core or not.
fn signalled(signal: i32, dumped: bool) -> ExitStatus {
    ExitStatus::from_raw(signal | if dumped { CORE_DUMPED } else { 0 })
}

#[cfg(test)]
mod tests {
    use std::process::Child;

    use nix::sys::signal::Signal as Kill;
    use nix::sys::signal::kill;

    use super::*;
    use crate::executor::sleeper::exits_on_close;

    /// How long a test waits for a wait that should resolve, however loaded
    /// the machine.
    const PATIENCE: Duration = Duration::from_secs(60);

    /// How many short-lived children a race test watches, so at least one
    /// is caught part way through its exit.
    const ATTEMPTS: usize = 300;

    fn pid(child: &Child) -> Pid {
        Pid::from_raw(i32::try_from(child.id()).unwrap())
    }

    /// Let the child exit only once `exit` has checked on it and gone to
    /// sleep, then wait for it to notice.
    async fn exit_after_first_check(
        child: &mut Child,
        exit: &mut LeaderExit,
    ) -> io::Result<ExitStatus> {
        let waiting = exit.wait();
        tokio::pin!(waiting);
        assert!(
            futures::poll!(&mut waiting).is_pending(),
            "the child exited before the wait checked on it"
        );
        drop(child.stdin.take());
        tokio::time::timeout(PATIENCE, waiting)
            .await
            .expect("the wait never saw the exit")
    }

    /// Nothing ever sends the signal this watch listens for, so only the poll
    /// can see the exit, as when a ptrace-based tool swallows `SIGCHLD`.
    #[tokio::test]
    async fn a_wait_resolves_when_no_sigchld_arrives() {
        let mut child = exits_on_close();
        let mut exit = LeaderExit::watch_on(pid(&child), SignalKind::user_defined2()).unwrap();

        let status = exit_after_first_check(&mut child, &mut exit).await.unwrap();

        assert_eq!(status.code(), Some(3));
        assert_eq!(child.wait().expect("the child is reaped").code(), Some(3));
    }

    #[tokio::test]
    async fn a_wait_leaves_the_child_for_its_reaper() {
        let mut child = exits_on_close();
        let mut exit = LeaderExit::watch(pid(&child)).unwrap();

        let seen = exit_after_first_check(&mut child, &mut exit).await.unwrap();

        let status = child
            .try_wait()
            .expect("the child is still there to reap")
            .expect("the child has exited");
        assert_eq!(status.code(), Some(3));
        assert_eq!(seen, status, "the wait saw another status than the reap");
    }

    /// A child that exited before it was watched cannot be registered for
    /// its exit on macOS, and still reports its status at once.
    #[tokio::test]
    async fn a_watch_begun_after_the_exit_reports_the_status_at_once() {
        let mut child = exits_on_close();
        let mut first = LeaderExit::watch(pid(&child)).unwrap();
        drop(child.stdin.take());
        let seen = tokio::time::timeout(PATIENCE, first.wait())
            .await
            .expect("the wait never saw the exit")
            .unwrap();

        let late = LeaderExit::watch(pid(&child))
            .unwrap()
            .probe
            .exit()
            .unwrap();

        assert_eq!(late, Some(seen));
        assert_eq!(child.wait().expect("the child is reaped").code(), Some(3));
    }

    /// macOS refuses to watch a child that is part way through exiting, a
    /// moment before it can be waited on, so a watch begun as a short-lived
    /// child exits must wait for the status rather than fail.
    #[tokio::test]
    async fn a_watch_begun_as_the_child_exits_waits_for_its_status() {
        for _ in 0..ATTEMPTS {
            let mut child = std::process::Command::new("sh")
                .args(["-c", "echo; exit 3"])
                .stdout(std::process::Stdio::piped())
                .spawn()
                .expect("the shell starts");
            let mut line = [0; 1];
            std::io::Read::read_exact(child.stdout.as_mut().expect("its stdout"), &mut line)
                .expect("the shell's last line");
            let watched = LeaderExit::watch(pid(&child));
            let status = match watched {
                Ok(mut exit) => tokio::time::timeout(PATIENCE, exit.wait())
                    .await
                    .expect("the wait never saw the exit"),
                Err(error) => Err(error),
            };
            let reaped = child.wait().expect("the child is reaped");
            assert_eq!(status.ok().and_then(|status| status.code()), Some(3));
            assert_eq!(reaped.code(), Some(3));
        }
    }

    #[tokio::test]
    async fn a_wait_reports_the_signal_that_killed_the_child() {
        let mut child = exits_on_close();
        let mut exit = LeaderExit::watch(pid(&child)).unwrap();

        kill(pid(&child), Kill::SIGTERM).unwrap();
        let status = tokio::time::timeout(PATIENCE, exit.wait())
            .await
            .expect("the wait never saw the exit")
            .unwrap();

        assert_eq!(status.signal(), Some(Kill::SIGTERM as i32));
        assert_eq!(
            child.wait().expect("the child is reaped").signal(),
            Some(Kill::SIGTERM as i32)
        );
    }

    #[tokio::test]
    async fn a_wait_does_not_resolve_while_the_child_runs() {
        let mut child = exits_on_close();
        let mut exit = LeaderExit::watch(pid(&child)).unwrap();

        let early = tokio::time::timeout(POLL_INTERVAL * 3, exit.wait()).await;

        kill(pid(&child), Kill::SIGKILL).unwrap();
        assert_eq!(
            child.wait().expect("the child is reaped").signal(),
            Some(Kill::SIGKILL as i32)
        );
        assert!(early.is_err(), "the wait resolved while the child ran");
    }
}
