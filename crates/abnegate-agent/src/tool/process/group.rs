use abnegate_exec::executor::ProcessGroup;

/// A child started as the leader of its own process group, and everything it
/// went on to start.
///
/// Killing only the child leaves its children running with the pipes and
/// files it shared still open, so every kill here goes to the whole group,
/// and dropping the handle is a kill: a call abandoned by a timeout leaves
/// nothing behind.
#[derive(Debug)]
pub(crate) struct Group {
    leader: Option<ProcessGroup>,
}

impl Group {
    /// The group led by `id`, which has to have been spawned with
    /// `process_group(0)`.
    pub(crate) fn led_by(id: Option<u32>) -> Self {
        Self {
            leader: id.and_then(|id| ProcessGroup::try_from(id).ok()),
        }
    }

    /// Kill every process still in the group. Later calls do nothing.
    pub(crate) fn kill(&mut self) {
        if let Some(leader) = self.leader.take()
            && let Err(error) = leader.kill()
        {
            tracing::warn!(group = leader.pgid(), %error, "Could not kill a process group");
        }
    }

    /// Kill every process still in the group, resolving once the kill has
    /// stopped repeating itself, so a leader reaped next is reaped only after
    /// the whole kill has landed. Later calls do nothing.
    pub(crate) async fn kill_until_gone(&mut self) {
        if let Some(leader) = self.leader.take()
            && let Err(error) = leader.kill_until_gone().await
        {
            tracing::warn!(group = leader.pgid(), %error, "Could not kill a process group");
        }
    }
}

impl Drop for Group {
    fn drop(&mut self) {
        self.kill();
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::process::CommandExt;
    use std::process::Command;
    use std::process::Stdio;
    use std::time::Duration;
    use std::time::Instant;

    use nix::sys::signal::Signal;
    use nix::sys::signal::kill;
    use nix::unistd::Pid;

    use super::Group;
    use crate::test_support::PATIENCE;

    /// The members of `group` still running; a zombie runs nothing.
    fn running(group: u32) -> Vec<i32> {
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
    fn dropping_a_group_reaches_a_child_forked_as_it_is_killed() {
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

        drop(Group::led_by(Some(leader)));
        shell.wait().expect("the shell is reaped");

        let gone = eventually(|| running(leader).is_empty());
        let survivors = running(leader);
        for pid in &survivors {
            let _ = kill(Pid::from_raw(*pid), Signal::SIGKILL);
        }
        assert!(gone, "the group's kill missed {survivors:?}");
    }
}
