//! Running a command to its end, or its time limit, with nothing left behind.

mod capture;
mod finished;
mod group;

pub(crate) use capture::Capture;
pub(crate) use finished::Finished;
pub(crate) use group::Group;

use std::process::Stdio;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;
use tokio::task::JoinHandle;
use tokio::time::{Instant, timeout, timeout_at};

use abnegate_exec::Proxy;

use super::{ToolContext, ToolError};

/// Most bytes kept from the start of each stream, and again from its end.
pub(crate) const MAX_CAPTURE_BYTES: usize = 64 * 1024;

/// How long the rest of a group may keep a pipe open once its leader has
/// exited, before it is killed.
///
/// A command that started something in the background and returned leaves
/// that child holding its output open, and waiting for the pipe to close
/// would wait for the child.
pub(crate) const GROUP_GRACE: Duration = Duration::from_secs(1);

/// How long reading may continue once the group has been killed, for a
/// member that escaped the group and still holds a pipe.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(1);

const READ_BUFFER_BYTES: usize = 8 * 1024;

/// `program`, set to see the context's environment and nothing else, with the
/// process-wide proxy policy applied last so no tool can route around it.
pub(crate) fn command(program: &str, context: &ToolContext) -> Command {
    let mut command = Command::new(program);
    command.env_clear();
    for (key, value) in &context.env {
        command.env(key, value);
    }
    Proxy::from_env().apply(&mut command);
    command
}

/// Run `command` in a process group of its own until it exits or `limit`
/// passes, reading both of its streams as it goes.
///
/// Whatever the leader leaves running is given [`GROUP_GRACE`] to finish and
/// then killed with it, so a call returns once its command has, and nothing
/// it started outlives it. Output is held to [`MAX_CAPTURE_BYTES`] from each
/// end of each stream.
pub(crate) async fn run(mut command: Command, limit: Duration) -> Result<Finished, ToolError> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .kill_on_drop(true);
    let mut child = command
        .spawn()
        .map_err(|error| ToolError::Execution(format!("Failed to execute: {error}")))?;
    let mut group = Group::led_by(child.id());
    let mut stdout = Stream::read(child.stdout.take());
    let mut stderr = Stream::read(child.stderr.take());

    let deadline = Instant::now() + limit;
    let exited = timeout_at(deadline, child.wait()).await;
    if exited.is_ok() {
        let _ = timeout(GROUP_GRACE, async {
            stdout.closed().await;
            stderr.closed().await;
        })
        .await;
    }
    group.kill();
    let _ = timeout(DRAIN_TIMEOUT, async {
        stdout.closed().await;
        stderr.closed().await;
    })
    .await;
    let (stdout, stderr) = (stdout.text(), stderr.text());

    match exited {
        Ok(Ok(status)) => Ok(Finished {
            status,
            stdout,
            stderr,
        }),
        Ok(Err(error)) => Err(ToolError::Execution(format!("Failed to execute: {error}"))),
        Err(_) => Err(ToolError::Execution(timed_out(limit, &stdout, &stderr))),
    }
}

fn timed_out(limit: Duration, stdout: &str, stderr: &str) -> String {
    let mut report = format!(
        "Command timed out after {} seconds and was killed",
        limit.as_secs()
    );
    for (name, text) in [("stdout", stdout), ("stderr", stderr)] {
        if !text.trim().is_empty() {
            report.push_str(&format!("\n\n{name}:\n{text}"));
        }
    }
    report
}

/// One pipe being read into a [`Capture`] on a task of its own.
struct Stream {
    capture: Arc<Mutex<Capture>>,
    reader: Option<JoinHandle<()>>,
}

impl Stream {
    fn read(pipe: Option<impl AsyncRead + Unpin + Send + 'static>) -> Self {
        let capture = Arc::new(Mutex::new(Capture::new(MAX_CAPTURE_BYTES)));
        let reader = pipe.map(|pipe| tokio::spawn(drain(pipe, Arc::clone(&capture))));
        Self { capture, reader }
    }

    /// Resolves once the pipe has reached its end.
    async fn closed(&mut self) {
        if let Some(reader) = self.reader.as_mut() {
            let _ = reader.await;
            self.reader = None;
        }
    }

    fn text(&self) -> String {
        self.capture
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .text()
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        if let Some(reader) = &self.reader {
            reader.abort();
        }
    }
}

async fn drain(mut pipe: impl AsyncRead + Unpin, capture: Arc<Mutex<Capture>>) {
    let mut buffer = vec![0; READ_BUFFER_BYTES];
    loop {
        match pipe.read(&mut buffer).await {
            Ok(0) | Err(_) => return,
            Ok(read) => capture
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(&buffer[..read]),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nix::sys::signal::kill;
    use nix::unistd::Pid;

    fn shell(line: &str) -> Command {
        let mut command = Command::new("sh");
        command.arg("-c").arg(line);
        command
    }

    async fn gone(pid: i32) -> bool {
        for _ in 0..300 {
            if kill(Pid::from_raw(pid), None).is_err() {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        false
    }

    /// `sleep` holds the pipe `sh` handed it, so waiting for the pipe to
    /// close waited out the sleep, and once the call gave up only `sh` was
    /// killed. The call now returns once `sh` has, and takes `sleep` with it.
    #[tokio::test]
    async fn a_child_left_running_in_the_background_neither_holds_the_call_nor_outlives_it() {
        let started = std::time::Instant::now();
        let finished = run(shell("sleep 30 & echo $!"), Duration::from_secs(3))
            .await
            .expect("the command finishes");

        assert!(finished.status.success());
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "the call waited for the background child: {:?}",
            started.elapsed()
        );
        let pid: i32 = finished.stdout.trim().parse().expect("the child's pid");
        assert!(gone(pid).await, "sleep {pid} outlived the call");
    }

    #[tokio::test]
    async fn a_command_past_its_limit_is_killed_with_everything_it_started() {
        let started = std::time::Instant::now();
        let error = run(
            shell("sleep 30 & echo $!; wait"),
            Duration::from_millis(500),
        )
        .await
        .expect_err("the command outlives its limit");

        assert!(started.elapsed() < Duration::from_secs(5));
        let message = error.to_string();
        assert!(message.contains("timed out"), "{message}");
        let pid: i32 = message
            .lines()
            .find_map(|line| line.trim().parse().ok())
            .expect("the partial output names the child");
        assert!(gone(pid).await, "sleep {pid} outlived the timeout");
    }

    /// `output()` held everything a command wrote. A command that writes
    /// without end now costs the two ends of each stream and nothing more.
    #[tokio::test]
    async fn a_flood_of_output_is_held_to_its_two_ends() {
        let finished = run(
            shell("head -c 20000000 /dev/zero | tr '\\0' x; echo; echo END"),
            Duration::from_secs(30),
        )
        .await
        .expect("the command finishes");

        assert!(finished.stdout.len() < 2 * MAX_CAPTURE_BYTES + 100);
        assert!(finished.stdout.starts_with("xxxx"));
        assert!(finished.stdout.trim_end().ends_with("END"));
        assert!(finished.stdout.contains("bytes of output dropped"));
    }

    #[tokio::test]
    async fn both_streams_and_the_status_come_back() {
        let finished = run(
            shell("echo out; echo err >&2; exit 3"),
            Duration::from_secs(5),
        )
        .await
        .expect("the command finishes");

        assert_eq!(finished.status.code(), Some(3));
        assert_eq!(finished.stdout, "out\n");
        assert_eq!(finished.stderr, "err\n");
    }
}
