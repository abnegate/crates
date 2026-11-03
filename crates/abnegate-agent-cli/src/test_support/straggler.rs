use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;
use std::time::Instant;

use tokio::signal::unix::SignalKind;
use tokio::signal::unix::signal;

use super::MARKER;
use super::PATIENCE;

/// Names the [`Straggler`] a re-run of this test binary acts out.
const BEHAVIOUR: &str = "ABNEGATE_AGENT_CLI_STRAGGLER";

/// Names the directory a [`Straggler`] keeps its records in.
const DIRECTORY: &str = "ABNEGATE_AGENT_CLI_STRAGGLER_DIRECTORY";

/// The file a straggler records its pid in, once it behaves as named.
const ESCAPED: &str = "escaped";

/// The file a [`Straggler::IgnoresTerminate`] adds a line to for each
/// SIGTERM it receives.
const TERMINATIONS: &str = "terminations";

/// The file a [`Straggler::ReportsLeader`] records its agent's state in.
const LEADER: &str = "leader";

/// The line a [`Straggler::IgnoresTerminate`] records for each SIGTERM.
pub(crate) const TERMINATION: &str = "TERM\n";

/// The status a [`Straggler::Interruptible`] exits with on SIGINT.
pub(crate) const INTERRUPTED: i32 = 7;

/// How long a straggler lives if no test stops it.
const LIFETIME: Duration = Duration::from_secs(300);

/// A process a fake agent starts: this test binary, re-run as [`straggle`].
/// Apple's platform binaries hide their environment from a marker's sweep,
/// and this binary is never one of them, so a sweep finds it whenever it
/// carries the run's marker. Each records its pid once it behaves as named.
///
/// All but [`Straggler::Interruptible`] leave the agent's process group for
/// a session of their own, out of reach of a kill of the group.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Straggler {
    /// Stays in the agent's group, and exits with [`INTERRUPTED`] on SIGINT
    /// however the process that started it left SIGINT's disposition, which
    /// a shell's `trap` cannot override once it was inherited as ignored.
    Interruptible,
    /// Sleeps, and exits on SIGTERM.
    Escapes,
    /// Sleeps, recording each SIGTERM it survives.
    IgnoresTerminate,
    /// On SIGTERM, records the state of the agent that started it, as `ps`
    /// reports it, and exits.
    ReportsLeader,
}

impl Straggler {
    const ALL: &[Self] = &[
        Self::Interruptible,
        Self::Escapes,
        Self::IgnoresTerminate,
        Self::ReportsLeader,
    ];

    fn as_str(self) -> &'static str {
        match self {
            Self::Interruptible => "interruptible",
            Self::Escapes => "escapes",
            Self::IgnoresTerminate => "ignores-terminate",
            Self::ReportsLeader => "reports-leader",
        }
    }

    fn parse(name: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|straggler| straggler.as_str() == name)
    }

    /// Lines of a fake agent's script that leave this straggler behind,
    /// keeping its records in `directory`, and go on once it has escaped.
    /// When `holding_output`, it keeps the agent's stdout open without
    /// writing to it.
    pub(crate) fn script(self, directory: &Path, holding_output: bool) -> String {
        let output = if holding_output { "3>&1 " } else { "" };
        format!(
            "{} </dev/null {output}>/dev/null 2>&1 &\nwhile [ ! -s '{}' ]; do sleep 0.05; done\n",
            self.command(directory),
            Self::announcement(directory).display(),
        )
    }

    /// A fake agent's script that becomes this straggler, keeping its
    /// records in `directory`.
    pub(crate) fn exec(self, directory: &Path) -> String {
        format!("exec env {} >/dev/null 2>&1", self.command(directory))
    }

    /// The file a straggler keeping its records in `directory` creates once
    /// it behaves as named.
    pub(crate) fn announcement(directory: &Path) -> PathBuf {
        directory.join(ESCAPED)
    }

    fn command(self, directory: &Path) -> String {
        format!(
            "{BEHAVIOUR}={} {DIRECTORY}='{}' '{}' --exact {} --include-ignored --nocapture",
            self.as_str(),
            directory.display(),
            executable().display(),
            name(),
        )
    }

    /// The pid the straggler recorded in `directory`.
    pub(crate) fn pid(directory: &Path) -> u32 {
        std::fs::read_to_string(directory.join(ESCAPED))
            .expect("the straggler's pid")
            .trim()
            .parse()
            .expect("a pid")
    }

    /// What a [`Straggler::IgnoresTerminate`] in `directory` recorded.
    pub(crate) fn terminations(directory: &Path) -> String {
        std::fs::read_to_string(directory.join(TERMINATIONS)).unwrap_or_default()
    }

    /// The state a [`Straggler::ReportsLeader`] in `directory` recorded.
    pub(crate) fn leader(directory: &Path) -> String {
        std::fs::read_to_string(directory.join(LEADER))
            .unwrap_or_default()
            .trim()
            .to_string()
    }

    fn perform(self, directory: &Path) {
        let leader = std::os::unix::process::parent_id();
        if self != Self::Interruptible {
            nix::unistd::setsid().expect("a session of its own");
        }
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a runtime")
            .block_on(self.behave(directory, leader));
    }

    async fn behave(self, directory: &Path, leader: u32) {
        let terminations = match self {
            Self::Escapes => None,
            Self::Interruptible => Some(signal(SignalKind::interrupt()).expect("a SIGINT handler")),
            Self::IgnoresTerminate | Self::ReportsLeader => {
                Some(signal(SignalKind::terminate()).expect("a SIGTERM handler"))
            }
        };
        announce(directory);
        let Some(mut terminations) = terminations else {
            tokio::time::sleep(LIFETIME).await;
            return;
        };
        let lifetime = tokio::time::sleep(LIFETIME);
        tokio::pin!(lifetime);
        loop {
            tokio::select! {
                () = &mut lifetime => return,
                _ = terminations.recv() => {}
            }
            if self == Self::Interruptible {
                std::process::exit(INTERRUPTED);
            }
            if self == Self::ReportsLeader {
                let state = Command::new("ps")
                    .args(["-o", "stat=", "-p", &leader.to_string()])
                    .env_remove(MARKER)
                    .output()
                    .expect("ps lists the agent");
                std::fs::write(directory.join(LEADER), state.stdout).expect("the agent's state");
                return;
            }
            record(&directory.join(TERMINATIONS));
        }
    }
}

fn announce(directory: &Path) {
    let announcing = directory.join(format!("{ESCAPED}.part"));
    std::fs::write(&announcing, std::process::id().to_string()).expect("the straggler's pid");
    std::fs::rename(&announcing, directory.join(ESCAPED)).expect("the straggler's pid");
}

fn record(path: &Path) {
    use std::io::Write;

    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .expect("the termination record");
    file.write_all(TERMINATION.as_bytes())
        .expect("a termination");
}

/// Whether `pid` is running. A zombie runs nothing: whoever reaps it may
/// never get to it.
fn is_running(pid: u32) -> bool {
    let output = Command::new("ps")
        .args(["-o", "stat=", "-p", &pid.to_string()])
        .output()
        .expect("ps lists processes");
    let state = String::from_utf8_lossy(&output.stdout);
    let state = state.trim();
    !state.is_empty() && !state.starts_with('Z')
}

/// Whether `pid` stops running within [`PATIENCE`].
pub(crate) fn exits_within(pid: u32) -> bool {
    assert!(
        is_running(std::process::id()),
        "ps must report live processes, or every exit check passes"
    );
    let start = Instant::now();
    while start.elapsed() < PATIENCE {
        if !is_running(pid) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    !is_running(pid)
}

/// Whether `pid` is still running, killing it if so.
pub(crate) fn stop_if_running(pid: u32) -> bool {
    let running = is_running(pid);
    if running {
        let _ = Command::new("kill").args(["-9", &pid.to_string()]).status();
    }
    running
}

fn executable() -> PathBuf {
    std::env::current_exe().expect("the test binary")
}

fn name() -> String {
    let path = module_path!();
    let module = path.split_once("::").map_or(path, |(_, module)| module);
    format!("{module}::straggle")
}

#[test]
#[ignore = "a process the lifecycle tests leave behind, not a test of its own"]
fn straggle() {
    let straggler = std::env::var(BEHAVIOUR)
        .ok()
        .as_deref()
        .and_then(Straggler::parse);
    let directory = std::env::var_os(DIRECTORY).map(PathBuf::from);
    if let (Some(straggler), Some(directory)) = (straggler, directory) {
        straggler.perform(&directory);
    }
}
