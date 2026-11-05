#[cfg(not(target_os = "linux"))]
use std::time::Duration;
#[cfg(not(target_os = "linux"))]
use std::time::Instant;

use nix::errno::Errno;
use nix::sys::signal::Signal;
use nix::sys::signal::killpg;
use nix::unistd::Pid;

/// The pause between one SIGKILL to a group and the next.
#[cfg(not(target_os = "linux"))]
const ROUND: Duration = Duration::from_millis(1);

/// How long a kill goes on repeating itself for a group that still answers.
/// A group stops answering as soon as its members are gone, so this bounds
/// only a member that cannot die; on a loaded host, members already sent
/// SIGKILL can go on answering for a second or more.
#[cfg(not(target_os = "linux"))]
const BUDGET: Duration = Duration::from_secs(10);

/// The name of the thread that repeats a kill after the group is dropped.
#[cfg(not(target_os = "linux"))]
const PURSUIT: &str = "abnegate-vcs-group-kill";

/// The process group a git command and every helper it starts run in, killed
/// when the command is abandoned -- timed out, or its future dropped -- and
/// left alone once it has been waited for, since its identifier is then free
/// for the system to hand to another group.
///
/// The group remains separate from the server so cancellation cannot signal
/// another task. Drop covers timeout and future cancellation, not the server
/// being killed; abrupt process death requires the hosting supervisor to tear
/// down its group.
pub(crate) struct Group {
    id: Pid,
    armed: bool,
    kill: fn(Pid) -> nix::Result<()>,
}

impl Group {
    pub(crate) fn new(id: Pid) -> Self {
        Self {
            id,
            armed: true,
            kill: terminate,
        }
    }

    /// A group torn down by `kill` in place of SIGKILL.
    #[cfg(test)]
    fn killed_by(id: Pid, kill: fn(Pid) -> nix::Result<()>) -> Self {
        Self {
            id,
            armed: true,
            kill,
        }
    }

    /// The command was waited for, so nothing is left to tear down.
    pub(crate) fn disarm(&mut self) {
        self.armed = false;
    }

    /// Repeat the kill on a thread of its own, so a drop returns at once
    /// however long the group goes on answering. Where no thread can be
    /// started the kill is repeated here instead, since a member the first
    /// kill missed would otherwise outlive the group.
    #[cfg(not(target_os = "linux"))]
    fn pursue(&self) {
        let (id, kill) = (self.id, self.kill);
        let pursuit = move || kill_until_gone(id, kill);
        if let Err(error) = std::thread::Builder::new()
            .name(PURSUIT.to_string())
            .spawn(pursuit)
        {
            tracing::warn!(%error, group = id.as_raw(), "Could not start the Git process group's kill thread");
            pursuit();
        }
    }

    /// Linux restarts a fork that a group signal interrupts, so its child is
    /// never left out of a kill.
    #[cfg(target_os = "linux")]
    fn pursue(&self) {}
}

/// SIGKILL to every live member of the group `id`.
fn terminate(id: Pid) -> nix::Result<()> {
    killpg(id, Signal::SIGKILL)
}

/// Repeat `kill` until it finds no live member -- `ESRCH` once the group is
/// empty, `EPERM` once only its unreaped zombies are left -- or the budget
/// runs out. Outside Linux a group signal reaches only the members present
/// when it lands, so a child a member was forking at that instant would
/// survive a single kill.
#[cfg(not(target_os = "linux"))]
fn kill_until_gone(id: Pid, kill: fn(Pid) -> nix::Result<()>) {
    let deadline = Instant::now() + BUDGET;
    while Instant::now() < deadline {
        std::thread::sleep(ROUND);
        if kill(id).is_err() {
            return;
        }
    }
}

impl Drop for Group {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        match (self.kill)(self.id) {
            Ok(()) => self.pursue(),
            Err(Errno::ESRCH) => {}
            Err(error) => {
                tracing::warn!(%error, group = self.id.as_raw(), "Could not terminate Git process group");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::process::CommandExt;
    use std::process::Command;
    use std::process::Stdio;
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;
    use std::time::Duration;
    use std::time::Instant;

    use nix::sys::signal::Signal;
    use nix::sys::signal::kill;
    use nix::unistd::Pid;

    use super::Group;

    /// How long the test waits for the group to start or to go.
    const PATIENCE: Duration = Duration::from_secs(60);

    /// How long a drop may take, a tenth of the budget the kill repeats for.
    const PROMPT: Duration = Duration::from_secs(1);

    /// How many kills [`answering`] has answered.
    static ANSWERED: AtomicUsize = AtomicUsize::new(0);

    /// A kill answered as a group with a member that cannot die answers it.
    fn answering(_group: Pid) -> nix::Result<()> {
        ANSWERED.fetch_add(1, Ordering::SeqCst);
        Ok(())
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
        let mut shell = Command::new("sh")
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

        drop(Group::new(Pid::from_raw(leader)));
        shell.wait().expect("the shell is reaped");

        let gone = eventually(|| running(leader).is_empty());
        let survivors = running(leader);
        for pid in &survivors {
            let _ = kill(Pid::from_raw(*pid), Signal::SIGKILL);
        }
        assert!(gone, "the group's kill missed {survivors:?}");
    }

    /// A drop runs where a command was abandoned, often on an async worker,
    /// so it sends the first kill and leaves the repeats to a thread of their
    /// own rather than waiting out a group that goes on answering.
    #[test]
    fn dropping_a_group_that_still_answers_returns_at_once() {
        let started = Instant::now();
        drop(Group::killed_by(Pid::from_raw(i32::MAX), answering));
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
}
