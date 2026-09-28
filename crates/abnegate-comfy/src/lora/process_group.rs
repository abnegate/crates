use std::io;

use abnegate_exec::executor;
use tokio::process::Child;
use tokio::process::Command;

/// The external trainer's process group: the shell its command runs in, and
/// everything that shell starts.
///
/// Killing only the shell leaves the trainer it started holding the GPU, so a
/// kill here reaches the whole group, and dropping the handle is a kill.
pub(super) struct ProcessGroup {
    group: Option<executor::ProcessGroup>,
}

impl ProcessGroup {
    /// Spawns `command` as the leader of a new process group.
    pub(super) fn spawn(command: &mut Command) -> io::Result<(Child, Self)> {
        command.process_group(0);
        let child = command.spawn()?;
        let group = Self {
            group: child
                .id()
                .and_then(|id| executor::ProcessGroup::try_from(id).ok()),
        };
        Ok((child, group))
    }

    /// Kills every process still in the group. Later calls do nothing.
    pub(super) fn kill(&mut self) {
        if let Some(group) = self.group.take()
            && let Err(error) = group.kill()
        {
            tracing::warn!(group = group.pgid(), %error, "could not kill the trainer's process group");
        }
    }
}

impl Drop for ProcessGroup {
    fn drop(&mut self) {
        self.kill();
    }
}

#[cfg(test)]
mod tests {
    use std::process::Stdio;
    use std::time::Duration;
    use std::time::Instant;

    use tokio::process::Command;

    use super::ProcessGroup;

    /// How long the test waits for the group to start or to go.
    const PATIENCE: Duration = Duration::from_secs(60);

    /// The members of `group` still running; a zombie runs nothing.
    fn running(group: u32) -> Vec<String> {
        let output = std::process::Command::new("ps")
            .args(["-A", "-o", "pid=,pgid=,stat="])
            .output()
            .expect("a process listing");
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(|line| {
                let mut fields = line.split_whitespace();
                let (pid, pgid, state) = (fields.next()?, fields.next()?, fields.next()?);
                (pgid.parse() == Ok(group) && !state.starts_with('Z')).then(|| pid.to_string())
            })
            .collect()
    }

    async fn eventually(condition: impl Fn() -> bool) -> bool {
        let started = Instant::now();
        while !condition() {
            if started.elapsed() > PATIENCE {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        true
    }

    /// Each of the shell's sleeps outlives [`PATIENCE`], so one the kill
    /// missed is still running when the test gives up waiting for the group.
    #[tokio::test]
    async fn dropping_the_group_reaches_a_child_forked_as_it_is_killed() {
        let (mut shell, group) = ProcessGroup::spawn(
            Command::new("sh")
                .args(["-c", "while :; do sleep 300 & done"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null()),
        )
        .expect("the shell starts");
        let leader = shell.id().expect("the shell's pid");
        assert!(
            eventually(|| running(leader).len() > 2).await,
            "the shell never started forking"
        );

        drop(group);
        shell.wait().await.expect("the shell is reaped");

        let gone = eventually(|| running(leader).is_empty()).await;
        let survivors = running(leader);
        for pid in &survivors {
            let _ = std::process::Command::new("kill")
                .args(["-9", pid])
                .status();
        }
        assert!(gone, "the group's kill missed {survivors:?}");
    }
}
