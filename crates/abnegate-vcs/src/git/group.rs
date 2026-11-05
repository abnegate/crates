use nix::errno::Errno;
use nix::sys::signal::Signal;
use nix::sys::signal::killpg;
use nix::unistd::Pid;

use crate::git::leader::Leader;
use crate::git::pursuit::Pursuit;

/// The process group a git command and every helper it starts run in, killed
/// when the command is abandoned -- timed out, or its future dropped -- and
/// left alone once it has been waited for, since its identifier is then free
/// for the system to hand to another group.
///
/// The group holds the command, its leader, so the leader is reaped only once
/// the group's teardown is over and the identifier cannot be reused while it
/// is still being signalled. A drop sends the first kill and hands the rest
/// of the teardown to a reaper thread shared by every group, so it returns at
/// once however long the group goes on answering.
///
/// The group remains separate from the server so cancellation cannot signal
/// another task. Drop covers timeout and future cancellation, not the server
/// being killed; abrupt process death requires the hosting supervisor to tear
/// down its group.
pub(crate) struct Group<L: Leader> {
    id: Pid,
    leader: Option<L>,
    armed: bool,
    kill: fn(Pid) -> nix::Result<()>,
}

impl<L: Leader> Group<L> {
    /// The group `id` that `leader` was started in, as its own first process.
    pub(crate) fn new(id: Pid, leader: L) -> Self {
        Self {
            id,
            leader: Some(leader),
            armed: true,
            kill: terminate,
        }
    }

    /// A group torn down by `kill` in place of SIGKILL.
    #[cfg(test)]
    fn killed_by(id: Pid, leader: L, kill: fn(Pid) -> nix::Result<()>) -> Self {
        Self {
            id,
            leader: Some(leader),
            armed: true,
            kill,
        }
    }

    /// The command the group was started for.
    pub(crate) fn leader(&mut self) -> &mut L {
        self.leader
            .as_mut()
            .expect("a group holds its leader until it is dropped")
    }

    /// The command was waited for, so nothing is left to tear down.
    pub(crate) fn disarm(&mut self) {
        self.armed = false;
    }
}

/// SIGKILL to every live member of the group `id`.
fn terminate(id: Pid) -> nix::Result<()> {
    killpg(id, Signal::SIGKILL)
}

impl<L: Leader> Drop for Group<L> {
    fn drop(&mut self) {
        let Some(leader) = self.leader.take() else {
            return;
        };
        if !self.armed {
            return;
        }
        let answered = match (self.kill)(self.id) {
            Ok(()) => true,
            Err(Errno::ESRCH | Errno::EPERM) => false,
            Err(error) => {
                tracing::warn!(%error, group = self.id.as_raw(), "Could not terminate Git process group");
                false
            }
        };
        Pursuit::new(self.id, self.kill, Box::new(leader), answered).hand_off();
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::process::CommandExt;
    use std::process::Command;
    use std::process::Stdio;
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;
    use std::sync::mpsc::Sender;
    use std::sync::mpsc::channel;
    use std::thread::ThreadId;
    use std::time::Duration;
    use std::time::Instant;

    use nix::errno::Errno;
    use nix::sys::signal::Signal;
    use nix::sys::signal::kill;
    use nix::unistd::Pid;

    use super::Group;
    use crate::git::leader::Leader;

    /// How long the test waits for the group to start or to go.
    const PATIENCE: Duration = Duration::from_secs(60);

    /// How long a drop may take, a tenth of the budget the kill repeats for.
    const PROMPT: Duration = Duration::from_secs(1);

    /// How long a test watches for a kill sent after the group stopped
    /// answering: many rounds of the reaper.
    const SETTLING: Duration = Duration::from_millis(100);

    /// How many kills a group in [`a_group_is_reaped_only_once_it_stops_answering`]
    /// answers before it is empty.
    const ANSWERS: usize = 2;

    /// The kills sent before the group stops answering and its leader is
    /// reaped: the first alone where Linux repeats none, and otherwise every
    /// answered one and the one that found the group empty.
    #[cfg(target_os = "linux")]
    const KILLS: usize = 1;
    #[cfg(not(target_os = "linux"))]
    const KILLS: usize = ANSWERS + 1;

    /// How many kills [`answering`] has answered.
    static ANSWERED: AtomicUsize = AtomicUsize::new(0);

    /// How many kills [`emptying`] has been sent.
    static EMPTYING: AtomicUsize = AtomicUsize::new(0);

    /// A kill answered as a group with a member that cannot die answers it.
    fn answering(_group: Pid) -> nix::Result<()> {
        ANSWERED.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    /// A kill answered by a group whose members are gone after [`ANSWERS`].
    fn emptying(_group: Pid) -> nix::Result<()> {
        match EMPTYING.fetch_add(1, Ordering::SeqCst) < ANSWERS {
            true => Ok(()),
            false => Err(Errno::ESRCH),
        }
    }

    /// A kill answered by a group whose members are already gone.
    fn empty(_group: Pid) -> nix::Result<()> {
        Err(Errno::ESRCH)
    }

    /// A leader reaped the first time it is asked, which reports the thread
    /// that reaped it and how many kills `sent` counted by then.
    struct Reported {
        sent: &'static AtomicUsize,
        report: Sender<(ThreadId, usize)>,
    }

    impl Leader for Reported {
        fn reaped(&mut self) -> bool {
            let _ = self.report.send((
                std::thread::current().id(),
                self.sent.load(Ordering::SeqCst),
            ));
            true
        }
    }

    /// A leader that has exited.
    struct Exited;

    impl Leader for Exited {
        fn reaped(&mut self) -> bool {
            true
        }
    }

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

    /// Each of the shell's sleeps outlives [`PATIENCE`], so one the kill
    /// missed is still running when the test gives up waiting for the group.
    #[test]
    fn dropping_an_armed_group_reaches_a_child_forked_as_it_is_killed() {
        let shell = Command::new("sh")
            .args(["-c", "while :; do sleep 300 & done"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()
            .expect("the shell starts");
        let leader = i32::try_from(shell.id()).expect("a pid");
        assert!(
            eventually(|| running(leader).len() > 2),
            "the shell never started forking"
        );

        drop(Group::new(Pid::from_raw(leader), shell));

        let gone = eventually(|| running(leader).is_empty());
        let survivors = running(leader);
        for pid in &survivors {
            let _ = kill(Pid::from_raw(*pid), Signal::SIGKILL);
        }
        assert!(gone, "the group's kill missed {survivors:?}");
    }

    /// A drop runs where a command was abandoned, often on an async worker,
    /// so it sends the first kill and leaves the repeats to the reaper rather
    /// than waiting out a group that goes on answering.
    #[test]
    fn dropping_a_group_that_still_answers_returns_at_once() {
        let started = Instant::now();
        drop(Group::killed_by(Pid::from_raw(i32::MAX), Exited, answering));
        let took = started.elapsed();
        let answered = ANSWERED.load(Ordering::SeqCst);

        assert!(took < PROMPT, "the drop took {took:?}");
        assert!(answered > 0, "the drop sent no kill");
        #[cfg(not(target_os = "linux"))]
        assert!(
            eventually(|| ANSWERED.load(Ordering::SeqCst) > answered),
            "the kill stopped repeating when the drop returned"
        );
    }

    /// The leader holds the group's identifier until it is reaped, so it is
    /// reaped only after the last kill, off the dropping thread, and nothing
    /// is signalled through the identifier once the group is empty.
    #[test]
    fn a_group_is_reaped_only_once_it_stops_answering() {
        let (report, reports) = channel();
        let leader = Reported {
            sent: &EMPTYING,
            report,
        };

        drop(Group::killed_by(Pid::from_raw(i32::MAX), leader, emptying));
        let (reaper, sent) = reports
            .recv_timeout(PATIENCE)
            .expect("the leader is reaped");
        std::thread::sleep(SETTLING);

        assert_ne!(
            reaper,
            std::thread::current().id(),
            "the drop reaped the leader itself"
        );
        assert_eq!(
            sent, KILLS,
            "the leader was reaped while its group still answered"
        );
        assert_eq!(
            EMPTYING.load(Ordering::SeqCst),
            KILLS,
            "a kill was sent after the group was empty"
        );
    }

    /// However many groups are dropped, one reaper thread tears them down.
    #[test]
    fn every_dropped_group_is_torn_down_on_one_thread() {
        static SENT: AtomicUsize = AtomicUsize::new(0);
        let (report, reports) = channel();

        for _ in 0..2 {
            let leader = Reported {
                sent: &SENT,
                report: report.clone(),
            };
            drop(Group::killed_by(Pid::from_raw(i32::MAX), leader, empty));
        }
        let first = reports
            .recv_timeout(PATIENCE)
            .expect("the first leader is reaped");
        let second = reports
            .recv_timeout(PATIENCE)
            .expect("the second leader is reaped");

        assert_ne!(
            first.0,
            std::thread::current().id(),
            "the drop reaped the leader itself"
        );
        assert_eq!(
            first.0, second.0,
            "each group was torn down on a thread of its own"
        );
    }

    /// Once a command has been waited for, the group is left alone.
    #[test]
    fn a_disarmed_group_sends_no_kill() {
        static SENT: AtomicUsize = AtomicUsize::new(0);
        fn counted(_group: Pid) -> nix::Result<()> {
            SENT.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        let mut group = Group::killed_by(Pid::from_raw(i32::MAX), Exited, counted);

        group.disarm();
        drop(group);

        assert_eq!(SENT.load(Ordering::SeqCst), 0);
    }
}
