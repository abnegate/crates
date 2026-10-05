use std::io::BufRead;
use std::io::BufReader;
use std::process::Child;
use std::process::Command;
use std::process::Stdio;
use std::time::Duration;
use std::time::Instant;

use nix::sys::signal::Signal;
use nix::sys::signal::kill;
use nix::unistd::Pid;

use super::Marker;

mod behaviour;

pub(super) use behaviour::Behaviour;

/// The variable the marker tests mark their processes with.
pub(super) const VARIABLE: &str = "ABNEGATE_EXEC_TEST_MARKER";

/// The file a [`Behaviour::CleansUp`] straggler creates on SIGTERM.
pub(super) const CLEANED: &str = "ABNEGATE_EXEC_TEST_CLEANED";

/// How long a test waits for a process to stop, however loaded the machine.
pub(super) const EXIT_DEADLINE: Duration = Duration::from_secs(30);

/// Names the [`Behaviour`] a straggler acts out.
const BEHAVIOUR: &str = "ABNEGATE_EXEC_TEST_STRAGGLER";

/// A process a marker test starts and sweeps: this test binary, re-run as
/// [`straggle`] with an environment of its own. Apple's platform binaries
/// hide their environment from a sweep, and this binary is never one of them.
/// Dropping it kills and reaps the process.
pub(super) struct Straggler {
    child: Child,
    pids: Vec<u32>,
}

impl Straggler {
    /// The unmarked command for a straggler acting out `behaviour`, with
    /// nothing else in its environment.
    pub(super) fn command(behaviour: Behaviour) -> Command {
        let mut command = Command::new(std::env::current_exe().expect("the test binary"));
        command
            .args(["--exact", &name(), "--include-ignored", "--nocapture"])
            .env_clear()
            .env(BEHAVIOUR, behaviour.as_str())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        command
    }

    /// [`command`](Self::command), carrying `marker`.
    pub(super) fn marked(behaviour: Behaviour, marker: &Marker) -> Command {
        let mut command = Self::command(behaviour);
        command.env(marker.variable(), marker.value());
        command
    }

    /// Start `command` and wait until it has announced `count` pids, its own
    /// first, which it does once it behaves as asked.
    pub(super) fn start(command: &mut Command, count: usize) -> Self {
        let mut child = command.spawn().expect("the straggler starts");
        let output = BufReader::new(child.stdout.take().expect("the straggler's output"));
        let pids = output
            .lines()
            .map(|line| line.expect("the straggler's output reads"))
            .filter_map(|line| line.trim().parse().ok())
            .take(count)
            .collect::<Vec<u32>>();
        assert_eq!(
            pids.len(),
            count,
            "the straggler stopped before it announced itself"
        );
        Self { child, pids }
    }

    pub(super) fn pid(&self) -> u32 {
        self.pids[0]
    }

    pub(super) fn pids(&self) -> &[u32] {
        &self.pids
    }

    /// Send SIGKILL without reaping, leaving a zombie behind.
    pub(super) fn kill(&mut self) {
        self.child.kill().expect("the straggler takes SIGKILL");
    }

    pub(super) fn is_running(&mut self) -> bool {
        self.child
            .try_wait()
            .expect("the straggler's status")
            .is_none()
    }

    /// Whether the straggler stops within [`EXIT_DEADLINE`].
    pub(super) fn stops(self) -> bool {
        exits_within(self.pid())
    }
}

impl Drop for Straggler {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Whether `pid` is running. A zombie runs nothing: whoever reaps it may
/// never get to it.
pub(super) fn is_running(pid: u32) -> bool {
    let output = Command::new("ps")
        .args(["-o", "stat=", "-p", &pid.to_string()])
        .output()
        .expect("ps lists processes");
    let state = String::from_utf8_lossy(&output.stdout);
    let state = state.trim();
    !state.is_empty() && !state.starts_with('Z')
}

/// Whether `pid` stops running within [`EXIT_DEADLINE`].
pub(super) fn exits_within(pid: u32) -> bool {
    assert!(
        is_running(std::process::id()),
        "ps must report live processes, or every exit check passes"
    );
    let start = Instant::now();
    while start.elapsed() < EXIT_DEADLINE {
        if !is_running(pid) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    !is_running(pid)
}

/// Whether `pid` is still running, killing it if so.
pub(super) fn stop_if_running(pid: u32) -> bool {
    let running = is_running(pid);
    if running {
        let _ = kill(Pid::from_raw(pid.cast_signed()), Signal::SIGKILL);
    }
    running
}

fn name() -> String {
    let path = module_path!();
    let module = path.split_once("::").map_or(path, |(_, module)| module);
    format!("{module}::straggle")
}

#[test]
#[ignore = "a process the marker tests start, not a test of its own"]
fn straggle() {
    if let Some(behaviour) = std::env::var(BEHAVIOUR)
        .ok()
        .as_deref()
        .and_then(Behaviour::parse)
    {
        behaviour.perform();
    }
}
