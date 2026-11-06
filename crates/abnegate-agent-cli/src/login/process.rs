use std::io;
use std::path::Path;
use std::process::ExitStatus;
use std::process::Stdio;

use abnegate_exec::executor::GRACE_PERIOD;
use abnegate_exec::executor::ProcessGroup;
use tokio::process::Child;
use tokio::process::ChildStderr;
use tokio::process::ChildStdout;
use tokio::process::Command;
use tokio::time::timeout;

use crate::login::Cli;
use crate::login::LoginError;

/// A sign-in CLI in a process group of its own, so that stopping it stops
/// everything it forked.
///
/// The CLI is left unreaped until whatever it left in its group has been
/// killed, so no signal ever reaches a group that took over its id.
pub(crate) struct Process {
    child: Child,
    group: Option<ProcessGroup>,
    reaped: bool,
}

impl Process {
    /// `cli` run with `arguments` and exactly the environment it is given,
    /// with nothing on its stdin, its output piped, in a process group of its
    /// own.
    pub(crate) fn command(cli: &Cli, arguments: &[&str]) -> Command {
        let mut command = Command::new(cli.executable());
        command
            .args(arguments)
            .env_clear()
            .envs(cli.variables())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .process_group(0);
        command
    }

    /// Starts `command`, which must lead a process group of its own, as
    /// [`Process::command`] does.
    pub(crate) fn spawn(mut command: Command, executable: &Path) -> Result<Self, LoginError> {
        let child = command.spawn().map_err(|error| LoginError::Unavailable {
            executable: executable.to_path_buf(),
            reason: error.to_string(),
        })?;
        let group = child.id().and_then(|pid| ProcessGroup::try_from(pid).ok());
        Ok(Self {
            child,
            group,
            reaped: false,
        })
    }

    pub(crate) fn stdout(&mut self) -> Option<ChildStdout> {
        self.child.stdout.take()
    }

    pub(crate) fn stderr(&mut self) -> Option<ChildStderr> {
        self.child.stderr.take()
    }

    /// Resolves once the CLI has exited, with the status it exited with,
    /// once whatever it left running in its group has been killed. When its
    /// exit cannot be watched, the whole group is killed instead. Cancel
    /// safe: a [`stop`](Self::stop) after a cancelled wait finishes the job.
    pub(crate) async fn wait(&mut self) -> io::Result<ExitStatus> {
        let Some(group) = self.group.clone() else {
            return self.reap().await;
        };
        match group.leader_exited().await {
            Ok(Some(status)) => {
                let _ = group.kill_until_gone().await;
                self.reap().await.map(|_| status)
            }
            Ok(None) => self.reap().await,
            Err(error) => {
                tracing::warn!(%error, "the sign-in CLI's exit could not be watched; killing its group");
                let _ = group.kill_until_gone().await;
                self.reap().await
            }
        }
    }

    /// Asks the whole group to exit, kills whatever is still running once
    /// the CLI has exited or the grace period is up, and reaps the CLI,
    /// returning the status it exited with, unless it was reaped before.
    pub(crate) async fn stop(&mut self) -> Option<ExitStatus> {
        if self.reaped {
            return None;
        }
        match self.group.clone() {
            Some(group) => {
                let _ = group.terminate();
                if let Ok(Err(error)) = timeout(GRACE_PERIOD, group.leader_exited()).await {
                    tracing::warn!(%error, "the sign-in CLI's exit could not be watched; killing its group");
                }
                let _ = group.kill_until_gone().await;
            }
            None => {
                let _ = self.child.start_kill();
            }
        }
        self.reap().await.ok()
    }

    async fn reap(&mut self) -> io::Result<ExitStatus> {
        if let Some(group) = &self.group {
            group.release();
        }
        let status = self.child.wait().await;
        self.reaped |= status.is_ok();
        status
    }
}

/// A caller that stops waiting, such as a request dropped mid-sign-in,
/// leaves the CLI and everything it forked running unless dropping the
/// handle ends them.
impl Drop for Process {
    fn drop(&mut self) {
        if self.reaped {
            return;
        }
        if let Some(group) = &self.group {
            let _ = group.kill_blocking();
            group.release();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::Duration;

    use nix::sys::wait::waitpid;
    use nix::unistd::Pid;
    use tempfile::TempDir;

    use super::*;
    use crate::test_support::cli::ended;
    use crate::test_support::cli::environment;
    use crate::test_support::cli::fake;
    use crate::test_support::cli::process;

    const ARGUMENTS: &str = "sign-in";
    const SETTLE: Duration = Duration::from_secs(10);

    /// A CLI that leaves a process behind in its group, deaf to `TERM`, and
    /// exits, reaped behind this crate's back so its exit cannot be watched.
    fn unwatchable(directory: &TempDir) -> (Process, PathBuf) {
        let forked = directory.path().join("forked");
        let executable = fake(
            directory,
            ARGUMENTS,
            &format!(
                "trap '' TERM\nsleep 120 >/dev/null 2>&1 &\necho $! > '{}'\nexit 0",
                forked.display()
            ),
        );
        let cli = Cli::new(&executable).with_environment(environment());
        let running = Process::spawn(Process::command(&cli, &[ARGUMENTS]), &executable)
            .expect("the CLI to start");
        let leader = running.child.id().expect("the CLI's process id");
        waitpid(
            Pid::from_raw(i32::try_from(leader).expect("a process id")),
            None,
        )
        .expect("the CLI reaped behind the crate's back");
        (running, forked)
    }

    #[tokio::test]
    async fn a_cli_whose_exit_cannot_be_watched_has_its_group_killed_once_waited_for() {
        let directory = TempDir::new().expect("a directory for the fake CLI");
        let (mut running, forked) = unwatchable(&directory);

        let waited = timeout(SETTLE, running.wait()).await;

        assert!(waited.is_ok(), "the wait outlived the CLI's group");
        assert!(
            ended(process(&forked)),
            "a process the CLI forked was left running"
        );
    }

    #[tokio::test]
    async fn a_cli_whose_exit_cannot_be_watched_has_its_group_killed_once_stopped() {
        let directory = TempDir::new().expect("a directory for the fake CLI");
        let (mut running, forked) = unwatchable(&directory);

        let stopped = timeout(SETTLE, running.stop()).await;

        assert!(stopped.is_ok(), "the stop outlived the CLI's group");
        assert!(
            ended(process(&forked)),
            "a process the CLI forked was left running"
        );
    }
}
