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
    /// once whatever it left running in its group has been killed. Cancel
    /// safe: a [`stop`](Self::stop) after a cancelled wait finishes the job.
    pub(crate) async fn wait(&mut self) -> io::Result<ExitStatus> {
        let exited = match &self.group {
            Some(group) => group.leader_exited().await,
            None => Ok(None),
        };
        match exited {
            Ok(Some(status)) => {
                if let Some(group) = &self.group {
                    let _ = group.kill_until_gone().await;
                }
                self.reap().await.map(|_| status)
            }
            Ok(None) => self.reap().await,
            Err(error) => {
                tracing::warn!(%error, "the sign-in CLI's exit could not be watched; waiting for it alone");
                self.group = None;
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
                    tracing::warn!(%error, "the sign-in CLI's exit could not be watched; killing it alone");
                    self.group = None;
                    let _ = self.child.start_kill();
                } else {
                    let _ = group.kill_until_gone().await;
                }
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
