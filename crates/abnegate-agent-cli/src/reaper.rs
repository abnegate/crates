//! Cleanup for a run whose future is dropped part way through.

use abnegate_exec::executor::ProcessGroup;

/// Kills a process group when dropped, unless disarmed first.
///
/// `kill_on_drop` reaches only the direct child, so a cancelled run would
/// otherwise orphan everything the agent forked.
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
    use std::process::Command;
    use std::process::Stdio;
    use std::time::Duration;
    use std::time::Instant;

    use abnegate_exec::executor::ProcessGroup;

    use super::Reaper;

    fn sleeper() -> std::process::Child {
        use std::os::unix::process::CommandExt;
        Command::new("sleep")
            .arg("30")
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
        drop(Reaper::new(Some(ProcessGroup::new(child.id()))));

        assert!(exits_within(&mut child, Duration::from_secs(5)));
    }

    #[test]
    fn a_disarmed_reaper_leaves_the_group_running() {
        let mut child = sleeper();
        let reaper = Reaper::new(Some(ProcessGroup::new(child.id())));
        assert!(reaper.group().is_some());
        reaper.disarm();

        assert!(!exits_within(&mut child, Duration::from_millis(200)));
        let _ = child.kill();
        let _ = child.wait();
    }
}
