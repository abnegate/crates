//! Process group management for clean process tree termination.
//!
//! On Unix systems, we create a new process group for each spawned command,
//! allowing us to send signals to the entire process tree when cancelling.

use std::io;
use std::process::ExitStatus;
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
use tokio::sync::watch;

use crate::error::ExecutorError;

use super::leader_exit::LeaderExit;

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod zombies;

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

/// The name of the thread a kill's repeats are sent from.
#[cfg(not(target_os = "linux"))]
const REPEATER: &str = "process-group-kill";

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
///
/// Once its leader has exited, a group whose members are all unreaped
/// zombies, or part way through their exits, has nothing left to stop, and
/// macOS refuses to signal it with `EPERM`. Every signal here counts that
/// refusal as delivered once it has seen that every member is dead, so
/// waiting for the leader and then killing what it left never fails on a
/// group the leader left empty, nor on one whose other members are still
/// exiting, while a refusal from a group with a member of another user's is
/// still reported.
#[derive(Debug, Clone)]
pub struct ProcessGroup {
    /// Process group ID (same as the leader process PID)
    pgid: i32,
    released: Arc<Mutex<bool>>,
    releases: Arc<watch::Sender<bool>>,
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
            releases: Arc::new(watch::Sender::new(false)),
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

    /// Send SIGINT to the entire process group, as a terminal's Ctrl-C would,
    /// so each member can wind down and clean up what it started.
    pub fn interrupt(&self) -> Result<(), ExecutorError> {
        self.signal(Signal::SIGINT)
    }

    /// Send SIGTERM to the entire process group.
    ///
    /// This will attempt to gracefully terminate all processes in the group.
    pub fn terminate(&self) -> Result<(), ExecutorError> {
        self.signal(Signal::SIGTERM)
    }

    /// Send SIGKILL to the entire process group, and go on sending it until no
    /// member is left alive to receive it, for at most a tenth of a second,
    /// without making the caller wait for any of it.
    ///
    /// Outside Linux a group signal reaches only the members present when it
    /// lands, so a child a member was forking at that instant would survive
    /// a single kill. The first SIGKILL is sent before this returns; the
    /// repeats are sent from a thread of their own, each one only while the
    /// group has not been released, so none is sent once it has been. That
    /// makes this safe to call from async code and from `Drop`. A caller
    /// that goes on to reap the leader itself should await
    /// [`kill_until_gone`](Self::kill_until_gone) instead, so that every
    /// repeat lands while the leader still holds the group's id.
    pub fn kill(&self) -> Result<(), ExecutorError> {
        self.living(Signal::SIGKILL, self.kill_repeating(Self::kill_again))
    }

    /// Send SIGKILL to the entire process group, and again until no member is
    /// left alive to receive it, for at most a tenth of a second, resolving
    /// once the last repeat has been sent.
    ///
    /// Between repeats it waits on the runtime's timer rather than blocking
    /// its thread, so nothing else on the runtime is held up.
    pub async fn kill_until_gone(&self) -> Result<(), ExecutorError> {
        self.signal(Signal::SIGKILL)?;
        #[cfg(not(target_os = "linux"))]
        {
            let deadline = tokio::time::Instant::now() + BUDGET;
            while tokio::time::Instant::now() < deadline {
                tokio::time::sleep(ROUND).await;
                if !self.round(&mut Self::kill_again) {
                    break;
                }
            }
        }
        Ok(())
    }

    /// Send SIGKILL to the entire process group, and again until no member is
    /// left alive to receive it, for at most a tenth of a second, on the
    /// caller's thread, returning once the last repeat has been sent.
    ///
    /// For `Drop`, where nothing can be awaited, just before a
    /// [`release`](Self::release): unlike [`kill`](Self::kill), it leaves no
    /// repeat on its way once it returns, so the caller can release the group
    /// and let its leader be reaped. It holds the caller's thread only while
    /// the group still answers, which is a round or two once SIGKILL has
    /// landed.
    pub fn kill_blocking(&self) -> Result<(), ExecutorError> {
        self.signal(Signal::SIGKILL)?;
        #[cfg(not(target_os = "linux"))]
        self.repeat(Self::kill_again);
        Ok(())
    }

    /// Resolve once the group's leader has exited, leaving it unreaped, with
    /// the status it exited with.
    ///
    /// Until the leader is reaped it keeps the group's id from being reused,
    /// so a caller can wait here, decide from the status what to stop,
    /// [`kill_until_gone`](Self::kill_until_gone) whatever the leader left
    /// running in its group, then [`release`](Self::release) the group and
    /// reap the leader. The leader must be a child of this process, and
    /// nothing may reap it during the wait.
    ///
    /// The exit is checked on every `SIGCHLD` and every quarter of a second,
    /// so it is seen even when its `SIGCHLD` never arrives. The future is
    /// cancel safe. It resolves as soon as any clone releases the group,
    /// before or during the wait, whatever the leader is doing: the leader of
    /// a released group is no longer this handle's to watch.
    ///
    /// Returns the leader's exit status, or `None` when a clone released the
    /// group before its leader's exit was seen.
    ///
    /// # Errors
    ///
    /// [`ExecutorError::ProcessGroupFailed`] when the leader cannot be
    /// watched: when no `SIGCHLD` listener can be registered, or when the
    /// leader is not a child of this process, or has been reaped.
    pub async fn leader_exited(&self) -> Result<Option<ExitStatus>, ExecutorError> {
        let mut releases = self.releases.subscribe();
        tokio::select! {
            biased;
            _ = releases.wait_for(|released| *released) => Ok(None),
            watched = self.watch_leader() => match watched {
                Ok(status) => Ok(Some(status)),
                Err(_) if self.is_released() => Ok(None),
                Err(error) => Err(ExecutorError::ProcessGroupFailed(format!(
                    "Failed to watch the leader of process group {}: {error}",
                    self.pgid
                ))),
            },
        }
    }

    /// Check if the process group is still running.
    ///
    /// Returns true if any process in the group is still alive, and false
    /// once the group has been released.
    pub fn is_alive(&self) -> bool {
        !*self.released() && kill(Pid::from_raw(-self.pgid), None).is_ok()
    }

    /// Stop every clone of this handle signalling the group, once any signal
    /// already on its way has been sent, the repeats a [`kill`](Self::kill)
    /// left running included. Call it just before reaping the leader, after
    /// which the group's identifier can name an unrelated group.
    pub fn release(&self) {
        *self.released() = true;
        self.releases.send_replace(true);
    }

    pub(crate) fn is_released(&self) -> bool {
        *self.released()
    }

    /// Whether `other` is a clone of this handle, rather than a handle built
    /// separately for the same id, as one for a group that reused the id of
    /// an earlier, released one would be.
    pub(crate) fn is_clone_of(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.released, &other.released)
    }

    /// `sent`, counting a group of zombies and members part way through
    /// their exits, which macOS refuses to signal with `EPERM`, as
    /// signalled: it has nothing left to stop. macOS refuses with `EPERM` too
    /// when a member belongs to another user, so the refusal is forgiven only
    /// once every member is seen to be dead.
    fn living(&self, signal: Signal, sent: Result<(), Errno>) -> Result<(), ExecutorError> {
        match sent {
            Err(Errno::EPERM) if self.only_dead() => Ok(()),
            sent => sent.map_err(|error| self.failure(signal, error)),
        }
    }

    #[cfg(target_os = "macos")]
    fn only_dead(&self) -> bool {
        zombies::only_dead(self.pgid)
    }

    /// Outside macOS a group of dead members takes a signal like any other.
    #[cfg(not(target_os = "macos"))]
    fn only_dead(&self) -> bool {
        false
    }

    fn kill_repeating(
        &self,
        again: impl FnMut(&Self) -> bool + Clone + Send + 'static,
    ) -> Result<(), Errno> {
        self.send(Signal::SIGKILL)?;
        self.repeat_in_background(again);
        Ok(())
    }

    /// Whether SIGKILL found a live member: not `ESRCH`, as once the group is
    /// empty, nor `EPERM`, as once only its unreaped zombies, and members
    /// part way through their exits, are left.
    fn kill_again(&self) -> bool {
        kill(Pid::from_raw(-self.pgid), Signal::SIGKILL).is_ok()
    }

    /// [`repeat`](Self::repeat) on a thread of its own, or on the caller's
    /// when no thread can be started.
    #[cfg(not(target_os = "linux"))]
    fn repeat_in_background(&self, again: impl FnMut(&Self) -> bool + Clone + Send + 'static) {
        let group = self.clone();
        let rounds = again.clone();
        let started = std::thread::Builder::new()
            .name(REPEATER.to_string())
            .spawn(move || group.repeat(rounds));
        if started.is_err() {
            self.repeat(again);
        }
    }

    /// Linux restarts a fork that a group signal interrupts, so its child is
    /// never left out of a kill.
    #[cfg(target_os = "linux")]
    fn repeat_in_background(&self, _again: impl FnMut(&Self) -> bool + Clone + Send + 'static) {}

    /// Repeat `again` until it finds no live member, or the group is
    /// released, or the budget runs out.
    #[cfg(not(target_os = "linux"))]
    fn repeat(&self, mut again: impl FnMut(&Self) -> bool) {
        let deadline = Instant::now() + BUDGET;
        while Instant::now() < deadline {
            std::thread::sleep(ROUND);
            if !self.round(&mut again) {
                return;
            }
        }
    }

    /// Send one repeat unless the group has been released, holding the guard
    /// while it is sent so that a release waits for it. Returns whether to go
    /// on.
    #[cfg(not(target_os = "linux"))]
    fn round(&self, again: &mut impl FnMut(&Self) -> bool) -> bool {
        let released = self.released();
        !*released && again(self)
    }

    async fn watch_leader(&self) -> io::Result<ExitStatus> {
        LeaderExit::watch(Pid::from_raw(self.pgid))?.wait().await
    }

    fn released(&self) -> MutexGuard<'_, bool> {
        self.released.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn signal(&self, signal: Signal) -> Result<(), ExecutorError> {
        self.living(signal, self.send(signal))
    }

    fn send(&self, signal: Signal) -> Result<(), Errno> {
        let released = self.released();
        if *released {
            return Ok(());
        }
        match kill(Pid::from_raw(-self.pgid), signal) {
            Ok(()) | Err(Errno::ESRCH) => Ok(()),
            Err(error) => Err(error),
        }
    }

    fn failure(&self, signal: Signal, error: Errno) -> ExecutorError {
        ExecutorError::ProcessGroupFailed(format!(
            "Failed to send {signal} to process group {}: {error}",
            self.pgid
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::process::CommandExt;
    use std::os::unix::process::ExitStatusExt;
    use std::process::Child;
    use std::process::Command;
    use std::process::Stdio;
    #[cfg(not(target_os = "linux"))]
    use std::sync::atomic::AtomicUsize;
    #[cfg(not(target_os = "linux"))]
    use std::sync::atomic::Ordering;
    #[cfg(not(target_os = "linux"))]
    use std::sync::mpsc;
    use std::time::Duration;
    use std::time::Instant;

    #[cfg(target_os = "macos")]
    use crate::executor::held_exit::HeldExit;
    use crate::executor::sleeper::Sleeper;
    use crate::executor::sleeper::exits_on_close;

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

    #[tokio::test]
    async fn kill_until_gone_reaches_a_child_forked_as_it_lands() {
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

        group.kill_until_gone().await.unwrap();
        shell.wait().expect("the shell is reaped");

        let gone = eventually(|| running(group.pgid()).is_empty());
        let survivors = running(group.pgid());
        for pid in &survivors {
            let _ = kill(Pid::from_raw(*pid), Signal::SIGKILL);
        }
        assert!(gone, "the kill missed {survivors:?}");
    }

    #[test]
    fn kill_blocking_reaches_a_child_forked_as_it_lands() {
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

        group.kill_blocking().unwrap();
        group.release();
        shell.wait().expect("the shell is reaped");

        let gone = eventually(|| running(group.pgid()).is_empty());
        let survivors = running(group.pgid());
        for pid in &survivors {
            let _ = kill(Pid::from_raw(*pid), Signal::SIGKILL);
        }
        assert!(gone, "the kill missed {survivors:?}");
    }

    /// A kill's repeats were sent on the caller's thread, which held a
    /// current-thread runtime, and every task on it, for up to a tenth of a
    /// second. Here the first repeat waits on a task of the caller's runtime,
    /// so it is answered only when the kill has left that runtime running.
    #[cfg(not(target_os = "linux"))]
    #[tokio::test(flavor = "current_thread")]
    async fn a_kill_leaves_a_current_thread_runtime_running() {
        let mut sleeper = Sleeper::start();
        let (proceed, proceeding) = mpsc::channel::<()>();
        let proceeding = Arc::new(Mutex::new(proceeding));
        let (answer, answered) = mpsc::channel::<bool>();
        let task = tokio::spawn(async move {
            let _ = proceed.send(());
        });

        sleeper
            .group()
            .kill_repeating(move |_| {
                let waited = proceeding
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .recv_timeout(PATIENCE);
                let _ = answer.send(waited.is_ok());
                false
            })
            .unwrap();
        task.await.unwrap();

        assert!(
            answered.recv_timeout(PATIENCE).expect("a repeat was sent"),
            "a repeat held the runtime's only thread"
        );
        assert_eq!(sleeper.wait(), Some(Signal::SIGKILL as i32));
    }

    /// Releasing the group waits for a repeat on its way and stops every one
    /// after it, so none can land on an identifier the reap has freed.
    #[cfg(not(target_os = "linux"))]
    #[test]
    fn no_repeat_is_sent_once_the_group_is_released() {
        let mut sleeper = Sleeper::start();
        let group = sleeper.group();
        let sent = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&sent);
        let (report, reported) = mpsc::channel::<()>();

        group
            .kill_repeating(move |_| {
                counted.fetch_add(1, Ordering::SeqCst);
                let _ = report.send(());
                true
            })
            .unwrap();
        reported.recv_timeout(PATIENCE).expect("a repeat was sent");
        group.release();
        let at_release = sent.load(Ordering::SeqCst);

        loop {
            match reported.recv_timeout(PATIENCE) {
                Ok(()) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => panic!("the repeats never ended"),
            }
        }
        assert_eq!(
            sent.load(Ordering::SeqCst),
            at_release,
            "a repeat was sent after the release"
        );
        assert_eq!(sleeper.wait(), Some(Signal::SIGKILL as i32));
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

    /// A member that joined the leader's group, the way a child the leader
    /// forks starts out.
    fn member_of(group: &ProcessGroup) -> Child {
        Command::new("sleep")
            .arg("30")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(group.pgid())
            .spawn()
            .expect("the member starts")
    }

    #[test]
    fn interrupt_reaches_every_member_with_sigint() {
        let mut sleeper = Sleeper::start();
        let group = sleeper.group();
        let mut member = member_of(&group);

        group.interrupt().unwrap();

        assert_eq!(sleeper.wait(), Some(Signal::SIGINT as i32));
        assert_eq!(
            member.wait().expect("the member is reaped").signal(),
            Some(Signal::SIGINT as i32)
        );
    }

    #[test]
    fn a_released_group_is_not_interrupted() {
        let mut sleeper = Sleeper::start();
        let group = sleeper.group();

        group.release();
        group.interrupt().unwrap();
        kill(Pid::from_raw(-group.pgid()), Signal::SIGTERM).unwrap();

        assert_eq!(sleeper.wait(), Some(Signal::SIGTERM as i32));
    }

    /// The order a caller that reaps the leader itself follows: wait for the
    /// leader, kill what it left in its group, then release and reap it.
    #[tokio::test]
    async fn leader_exited_leaves_the_group_to_kill_and_the_leader_to_reap() {
        let mut leader = exits_on_close();
        let group = ProcessGroup::try_from(leader.id()).expect("a child leading its own group");
        let mut member = member_of(&group);
        let exited = group.leader_exited();
        tokio::pin!(exited);
        assert!(
            futures::poll!(&mut exited).is_pending(),
            "the leader exit resolved while the leader ran"
        );

        drop(leader.stdin.take());
        let exited = tokio::time::timeout(PATIENCE, exited)
            .await
            .expect("the leader exit never resolved")
            .unwrap();

        assert_eq!(exited.and_then(|status| status.code()), Some(3));
        assert!(
            member.try_wait().unwrap().is_none(),
            "the member was stopped"
        );
        group.kill_until_gone().await.unwrap();
        assert_eq!(
            member.wait().expect("the member is reaped").signal(),
            Some(Signal::SIGKILL as i32)
        );
        group.release();
        let status = leader
            .try_wait()
            .expect("the leader is still there to reap")
            .expect("the leader has exited");
        assert_eq!(status.code(), Some(3));
    }

    /// Once its leader has exited, a group with no other member holds only
    /// the leader's unreaped zombie, which macOS refuses to signal with
    /// `EPERM`: the order a caller that reaps the leader itself follows must
    /// not fail on it.
    #[tokio::test]
    async fn a_group_left_with_only_its_unreaped_leader_takes_every_signal() {
        let mut leader = exits_on_close();
        let group = ProcessGroup::try_from(leader.id()).expect("a child leading its own group");
        drop(leader.stdin.take());
        tokio::time::timeout(PATIENCE, group.leader_exited())
            .await
            .expect("the leader exit never resolved")
            .unwrap();

        let killed = group.kill_until_gone().await;
        let signalled = [group.kill(), group.interrupt(), group.terminate()];

        group.release();
        let status = leader.wait().expect("the leader is reaped");
        assert!(killed.is_ok(), "{killed:?}");
        assert!(signalled.iter().all(Result::is_ok), "{signalled:?}");
        assert_eq!(status.code(), Some(3));
    }

    /// A member part way through its exit is past stopping, and macOS
    /// refuses with `EPERM` to signal a group left with only it and its
    /// leader's zombie, as a shutdown that interrupts a run and kills what
    /// the leader left finds it when the run's other processes exit on the
    /// interrupt alongside the leader.
    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn a_group_left_with_a_member_still_exiting_takes_every_signal() {
        let mut leader = exits_on_close();
        let group = ProcessGroup::try_from(leader.id()).expect("a child leading its own group");
        let exiting = HeldExit::join(&group);
        drop(leader.stdin.take());
        tokio::time::timeout(PATIENCE, group.leader_exited())
            .await
            .expect("the leader exit never resolved")
            .unwrap();
        let refused = group.send(Signal::SIGKILL);

        let killed = group.kill_until_gone().await;
        let signalled = [group.kill(), group.interrupt(), group.terminate()];

        drop(exiting);
        group.release();
        let status = leader.wait().expect("the leader is reaped");
        assert_eq!(
            refused,
            Err(Errno::EPERM),
            "the kernel did not refuse a group left with only its exiting member and zombie leader"
        );
        assert!(killed.is_ok(), "{killed:?}");
        assert!(signalled.iter().all(Result::is_ok), "{signalled:?}");
        assert_eq!(status.code(), Some(3));
    }

    /// macOS refuses a group signal with `EPERM` too when a member belongs
    /// to another user, and such a group still has members left to stop.
    #[test]
    fn a_refusal_from_a_group_with_a_live_member_is_reported() {
        let sleeper = Sleeper::start();
        let group = sleeper.group();

        let refused = group.living(Signal::SIGKILL, Err(Errno::EPERM));

        assert!(
            refused.is_err(),
            "a refusal from a running group was taken for a group of zombies"
        );
    }

    #[tokio::test]
    async fn a_release_during_the_wait_resolves_it_while_the_leader_runs() {
        let mut sleeper = Sleeper::start();
        let group = sleeper.group();
        let exited = group.leader_exited();
        tokio::pin!(exited);
        assert!(
            futures::poll!(&mut exited).is_pending(),
            "the leader exit resolved while the leader ran"
        );

        group.clone().release();

        assert!(
            matches!(
                futures::poll!(&mut exited),
                std::task::Poll::Ready(Ok(None))
            ),
            "the release did not end the wait"
        );
        kill(Pid::from_raw(-group.pgid()), Signal::SIGTERM).unwrap();
        assert_eq!(
            sleeper.wait(),
            Some(Signal::SIGTERM as i32),
            "the leader did not run on until the test stopped it"
        );
    }

    #[tokio::test]
    async fn leader_exited_resolves_at_once_for_a_released_group() {
        let mut sleeper = Sleeper::start();
        let group = sleeper.group();
        group.release();

        let exited = group.leader_exited();
        tokio::pin!(exited);

        assert!(
            matches!(
                futures::poll!(&mut exited),
                std::task::Poll::Ready(Ok(None))
            ),
            "a released group's leader was watched"
        );

        kill(Pid::from_raw(-group.pgid()), Signal::SIGTERM).unwrap();
        assert_eq!(sleeper.wait(), Some(Signal::SIGTERM as i32));
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
