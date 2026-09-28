//! Process group management for clean process tree termination.
//!
//! On Unix systems, we create a new process group for each spawned command,
//! allowing us to send signals to the entire process tree when cancelling.

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::MutexGuard;
use std::sync::PoisonError;
#[cfg(not(target_os = "linux"))]
use std::time::Duration;
#[cfg(not(target_os = "linux"))]
use std::time::Instant;

use nix::errno::Errno;
use nix::sys::signal::Signal;
use nix::sys::signal::kill;
use nix::unistd::Pid;
use nix::unistd::getpgrp;

use crate::error::ExecutorError;

/// The lowest process identifier that can lead a job's group. `0` names the
/// caller's own group and `1` is init, and `kill(-1, ...)` signals every
/// process the caller may signal.
const LOWEST_GROUP: u32 = 2;

/// The pause between one SIGKILL to a group and the next.
#[cfg(not(target_os = "linux"))]
const ROUND: Duration = Duration::from_millis(1);

/// How long a kill goes on repeating itself for a group that still answers.
#[cfg(not(target_os = "linux"))]
const BUDGET: Duration = Duration::from_millis(100);

/// A handle to a process group for signal management.
///
/// Built from the pid of a child that leads its own group, spawned with
/// `process_group(0)` or `setsid`. Construction refuses every identifier for
/// which `kill(-pgid, ...)` would reach more than one job's group -- `0`, `1`,
/// the caller's own group, and anything past `i32::MAX` -- but does not check
/// that the pid really leads a group. Signal a group only while its leader is
/// unreaped, or while it is known to still have members: once it is empty its
/// id may belong to an unrelated group.
///
/// Clones share one handle. The executor releases a job's group just before
/// it reaps the leader, and from then on no clone -- such as the one a
/// [`JobRegistry`](crate::job::JobRegistry) holds -- signals anything.
#[derive(Debug, Clone)]
pub struct ProcessGroup {
    /// Process group ID (same as the leader process PID)
    pgid: i32,
    released: Arc<Mutex<bool>>,
}

impl TryFrom<u32> for ProcessGroup {
    type Error = ExecutorError;

    fn try_from(pid: u32) -> Result<Self, Self::Error> {
        let pgid = i32::try_from(pid)
            .ok()
            .filter(|_| pid >= LOWEST_GROUP)
            .filter(|pgid| *pgid != getpgrp().as_raw())
            .ok_or(ExecutorError::InvalidProcessGroup(pid))?;
        Ok(Self {
            pgid,
            released: Arc::new(Mutex::new(false)),
        })
    }
}

impl ProcessGroup {
    /// The group led by `pid`.
    ///
    /// # Panics
    ///
    /// When `pid` is `0`, `1`, the caller's own process group, or greater
    /// than `i32::MAX`, none of which is a pid a spawned child can have.
    /// [`ProcessGroup::try_from`] reports the same cases as an error.
    pub fn new(pid: u32) -> Self {
        Self::try_from(pid).unwrap_or_else(|error| panic!("{error}"))
    }

    /// Get the process group ID
    pub fn pgid(&self) -> i32 {
        self.pgid
    }

    /// Send SIGTERM to the entire process group.
    ///
    /// This will attempt to gracefully terminate all processes in the group.
    pub fn terminate(&self) -> Result<(), ExecutorError> {
        self.signal(Signal::SIGTERM)
    }

    /// Send SIGKILL to the entire process group, and again until no member is
    /// left alive to receive it, for at most a tenth of a second.
    ///
    /// Outside Linux a group signal reaches only the members present when it
    /// lands, so a child a member was forking at that instant would survive
    /// a single kill.
    pub fn kill(&self) -> Result<(), ExecutorError> {
        self.signal(Signal::SIGKILL)?;
        self.kill_until_gone();
        Ok(())
    }

    /// Check if the process group is still running.
    ///
    /// Returns true if any process in the group is still alive, and false
    /// once the group has been released.
    pub fn is_alive(&self) -> bool {
        !*self.released() && kill(Pid::from_raw(-self.pgid), None).is_ok()
    }

    /// Stop every clone of this handle signalling the group, once any signal
    /// already on its way has been sent. Call it just before reaping the
    /// leader, after which the group's identifier can name an unrelated group.
    pub(crate) fn release(&self) {
        *self.released() = true;
    }

    #[cfg(test)]
    pub(crate) fn is_released(&self) -> bool {
        *self.released()
    }

    /// Repeat SIGKILL until it finds no live member -- `ESRCH` once the group
    /// is empty, `EPERM` once only its unreaped zombies are left -- or the
    /// group is released, or the budget runs out.
    #[cfg(not(target_os = "linux"))]
    fn kill_until_gone(&self) {
        let deadline = Instant::now() + BUDGET;
        while Instant::now() < deadline {
            std::thread::sleep(ROUND);
            let released = self.released();
            if *released || kill(Pid::from_raw(-self.pgid), Signal::SIGKILL).is_err() {
                return;
            }
        }
    }

    /// Linux restarts a fork that a group signal interrupts, so its child is
    /// never left out of a kill.
    #[cfg(target_os = "linux")]
    fn kill_until_gone(&self) {}

    fn released(&self) -> MutexGuard<'_, bool> {
        self.released.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn signal(&self, signal: Signal) -> Result<(), ExecutorError> {
        let released = self.released();
        if *released {
            return Ok(());
        }
        match kill(Pid::from_raw(-self.pgid), signal) {
            Ok(()) | Err(Errno::ESRCH) => Ok(()),
            Err(error) => Err(ExecutorError::ProcessGroupFailed(format!(
                "Failed to send {signal} to process group {}: {error}",
                self.pgid
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::process::CommandExt;
    use std::process::Command;
    use std::process::Stdio;
    use std::time::Duration;
    use std::time::Instant;

    use crate::executor::sleeper::Sleeper;

    use super::*;

    /// How long a test waits for a group to start or to go, however loaded
    /// the machine.
    const PATIENCE: Duration = Duration::from_secs(60);

    /// The members of `group` still running; a zombie runs nothing.
    fn running(group: i32) -> Vec<i32> {
        let output = Command::new("ps")
            .args(["-A", "-o", "pid=,pgid=,stat="])
            .output()
            .expect("a process listing");
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(|line| {
                let mut fields = line.split_whitespace();
                let (pid, pgid, state) = (fields.next()?, fields.next()?, fields.next()?);
                (pgid.parse() == Ok(group) && !state.starts_with('Z'))
                    .then(|| pid.parse().ok())
                    .flatten()
            })
            .collect()
    }

    /// Whether `condition` holds within [`PATIENCE`].
    fn eventually(condition: impl Fn() -> bool) -> bool {
        let started = Instant::now();
        while !condition() {
            if started.elapsed() > PATIENCE {
                return false;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        true
    }

    /// Each of its sleeps outlives [`PATIENCE`], so one a kill missed is
    /// still running when the test gives up waiting for the group to go.
    #[test]
    fn kill_reaches_a_child_forked_as_it_lands() {
        let mut shell = Command::new("sh")
            .args(["-c", "while :; do sleep 300 & done"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()
            .expect("the shell starts");
        let group = ProcessGroup::try_from(shell.id()).expect("a child leading its own group");
        assert!(
            eventually(|| running(group.pgid()).len() > 2),
            "the shell never started forking"
        );

        group.kill().unwrap();
        shell.wait().expect("the shell is reaped");

        let gone = eventually(|| running(group.pgid()).is_empty());
        let survivors = running(group.pgid());
        for pid in &survivors {
            let _ = kill(Pid::from_raw(*pid), Signal::SIGKILL);
        }
        assert!(gone, "the kill missed {survivors:?}");
    }

    #[test]
    fn a_pid_that_is_not_one_jobs_group_is_refused() {
        let own = u32::try_from(getpgrp().as_raw()).unwrap();
        for pid in [0, 1, own, i32::MAX as u32 + 1, u32::MAX] {
            assert!(
                matches!(
                    ProcessGroup::try_from(pid),
                    Err(ExecutorError::InvalidProcessGroup(refused)) if refused == pid
                ),
                "{pid} must not become a process group"
            );
        }
    }

    #[test]
    #[should_panic(expected = "Not a job's process group: 1")]
    fn new_panics_on_a_pid_that_is_not_one_jobs_group() {
        ProcessGroup::new(1);
    }

    #[test]
    fn new_accepts_a_child_leading_its_own_group() {
        let sleeper = Sleeper::start();

        assert_eq!(
            ProcessGroup::new(sleeper.pid()).pgid(),
            i32::try_from(sleeper.pid()).unwrap()
        );
    }

    #[test]
    fn a_child_leading_its_own_group_is_accepted() {
        let sleeper = Sleeper::start();

        let group = ProcessGroup::try_from(sleeper.pid()).unwrap();

        assert_eq!(group.pgid(), i32::try_from(sleeper.pid()).unwrap());
    }

    #[test]
    fn terminate_signals_the_group_with_sigterm() {
        let mut sleeper = Sleeper::start();

        sleeper.group().terminate().unwrap();

        assert_eq!(sleeper.wait(), Some(Signal::SIGTERM as i32));
    }

    #[test]
    fn kill_signals_the_group_with_sigkill() {
        let mut sleeper = Sleeper::start();

        sleeper.group().kill().unwrap();

        assert_eq!(sleeper.wait(), Some(Signal::SIGKILL as i32));
    }

    #[test]
    fn a_group_is_alive_until_its_last_process_is_reaped() {
        let mut sleeper = Sleeper::start();
        let group = sleeper.group();
        assert!(group.is_alive());

        group.kill().unwrap();
        sleeper.wait();

        assert!(!group.is_alive());
    }

    /// A released group's identifier may already name an unrelated group,
    /// which the sleeper stands in for: only the test's own SIGTERM may reach
    /// it, through a clone released by the original.
    #[test]
    fn a_released_group_signals_nothing() {
        let mut sleeper = Sleeper::start();
        let group = sleeper.group();
        let clone = group.clone();

        group.release();

        assert!(clone.is_released());
        assert!(!clone.is_alive());
        clone.kill().unwrap();
        clone.terminate().unwrap();
        kill(Pid::from_raw(-group.pgid()), Signal::SIGTERM).unwrap();
        assert_eq!(
            sleeper.wait(),
            Some(Signal::SIGTERM as i32),
            "a released group was signalled"
        );
    }

    #[test]
    fn signalling_a_group_that_has_gone_is_not_an_error() {
        let mut sleeper = Sleeper::start();
        let group = sleeper.group();
        group.kill().unwrap();
        sleeper.wait();

        assert!(group.terminate().is_ok());
        assert!(group.kill().is_ok());
    }
}
