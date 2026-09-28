//! Cleanup for a run whose future is dropped part way through.

use abnegate_exec::executor::ProcessGroup;

/// Kills a process group when dropped, unless disarmed first.
///
/// `kill_on_drop` reaches only the direct child, so a cancelled run would
/// otherwise orphan everything the agent forked.
///
/// A run holds this alongside the child and drops it first, so a cancelled
/// run kills the group while its leader is still unreaped and the group's id
/// cannot belong to anyone else. A run that failed after its leader exited
/// by itself kills the group once its output is drained; that reaches
/// stragglers safely because a live straggler keeps the id from being
/// reused, and an emptied group whose id was taken in the meantime is the
/// residual risk, which only a process handle the platform does not offer
/// here could close.
#[derive(Debug)]
pub(crate) struct Reaper {
    group: Option<ProcessGroup>,
}

impl Reaper {
    pub(crate) fn new(group: Option<ProcessGroup>) -> Self {
        Self { group }
    }

    pub(crate) fn group(&self) -> Option<&ProcessGroup> {
        self.group.as_ref()
    }

    /// Leave the group alone from here on.
    pub(crate) fn disarm(mut self) {
        self.group = None;
    }
}

impl Drop for Reaper {
    fn drop(&mut self) {
        if let Some(group) = &self.group {
            let _ = group.kill();
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

    use abnegate_exec::executor::ProcessGroup;

    use super::Reaper;
    use crate::test_support::PATIENCE;
    use crate::test_support::running;

    fn sleeper() -> std::process::Child {
        Command::new("sleep")
            .arg("120")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()
            .expect("sleep to start")
    }

    fn exits_within(child: &mut std::process::Child, limit: Duration) -> bool {
        let start = Instant::now();
        while start.elapsed() < limit {
            if child.try_wait().expect("a status").is_some() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    #[test]
    fn dropping_an_armed_reaper_kills_the_group() {
        let mut child = sleeper();
        drop(Reaper::new(Some(
            ProcessGroup::try_from(child.id()).unwrap(),
        )));

        assert!(
            exits_within(&mut child, PATIENCE),
            "dropping the reaper left its group running"
        );
    }

    fn eventually(condition: impl Fn() -> bool) -> bool {
        let start = Instant::now();
        while !condition() {
            if start.elapsed() > PATIENCE {
                return false;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        true
    }

    /// A cancelled run drops its reaper whatever the agent is doing, forking
    /// included. Each of the shell's sleeps outlives [`PATIENCE`], so one the
    /// kill missed is still running when the test gives up on the group.
    #[test]
    fn dropping_an_armed_reaper_reaches_a_child_forked_as_it_fires() {
        let mut shell = Command::new("sh")
            .args(["-c", "while :; do sleep 300 & done"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()
            .expect("the shell starts");
        let leader = shell.id();
        assert!(
            eventually(|| running(leader).len() > 2),
            "the shell never started forking"
        );

        drop(Reaper::new(Some(ProcessGroup::try_from(leader).unwrap())));
        shell.wait().expect("the shell is reaped");

        let gone = eventually(|| running(leader).is_empty());
        let survivors = running(leader);
        for pid in &survivors {
            let _ = Command::new("kill").args(["-9", pid]).status();
        }
        assert!(gone, "the reaper's kill missed {survivors:?}");
    }

    #[test]
    fn a_disarmed_reaper_leaves_the_group_running() {
        let mut child = sleeper();
        let reaper = Reaper::new(Some(ProcessGroup::try_from(child.id()).unwrap()));
        assert!(reaper.group().is_some());
        reaper.disarm();

        assert!(!exits_within(&mut child, Duration::from_millis(200)));
        let _ = child.kill();
        let _ = child.wait();
    }
}
