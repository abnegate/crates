use std::io::Write;
use std::process::Stdio;
use std::time::Duration;

use nix::unistd::setsid;
use tokio::runtime::Runtime;
use tokio::signal::unix::SignalKind;
use tokio::signal::unix::signal;

use super::CLEANED;
use super::Straggler;

/// How long a straggler lives if no test stops it.
const LIFETIME: Duration = Duration::from_secs(300);

/// What a [`Straggler`] does once it starts. Each one announces its pid only
/// once it behaves as named.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Behaviour {
    /// Sleeps in the test's own session.
    Stays,
    /// Moves to a session of its own and sleeps.
    Escapes,
    /// Escapes, and on SIGTERM creates the file named by [`CLEANED`] and
    /// exits.
    CleansUp,
    /// Escapes and ignores SIGTERM.
    IgnoresTerminate,
    /// Escapes, then starts a straggler that [`Stays`](Self::Stays) in its
    /// session without the test's environment, and announces that one's pid
    /// too.
    Forks,
}

impl Behaviour {
    const ALL: &[Self] = &[
        Self::Stays,
        Self::Escapes,
        Self::CleansUp,
        Self::IgnoresTerminate,
        Self::Forks,
    ];

    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Stays => "stays",
            Self::Escapes => "escapes",
            Self::CleansUp => "cleans-up",
            Self::IgnoresTerminate => "ignores-terminate",
            Self::Forks => "forks",
        }
    }

    pub(super) fn parse(name: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|behaviour| behaviour.as_str() == name)
    }

    pub(super) fn perform(self) {
        if self != Self::Stays {
            setsid().expect("a session of its own");
        }
        match self {
            Self::Stays | Self::Escapes => {
                announce();
                std::thread::sleep(LIFETIME);
            }
            Self::CleansUp => runtime().block_on(clean_up_on_terminate()),
            Self::IgnoresTerminate => runtime().block_on(ignore_terminate()),
            Self::Forks => {
                announce();
                let mut sibling = Straggler::command(Self::Stays)
                    .stdout(Stdio::inherit())
                    .spawn()
                    .expect("the sibling starts");
                std::thread::sleep(LIFETIME);
                let _ = sibling.kill();
                let _ = sibling.wait();
            }
        }
    }
}

async fn clean_up_on_terminate() {
    let mut terminate = signal(SignalKind::terminate()).expect("a SIGTERM handler");
    announce();
    if tokio::time::timeout(LIFETIME, terminate.recv())
        .await
        .is_ok()
    {
        std::fs::write(std::env::var_os(CLEANED).expect("a file to clean up"), b"")
            .expect("the clean-up file is written");
        std::process::exit(0);
    }
}

async fn ignore_terminate() {
    let mut terminate = signal(SignalKind::terminate()).expect("a SIGTERM handler");
    announce();
    let ignoring = async { while terminate.recv().await.is_some() {} };
    let _ = tokio::time::timeout(LIFETIME, ignoring).await;
}

fn runtime() -> Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a runtime")
}

fn announce() {
    let mut output = std::io::stdout().lock();
    writeln!(output, "{}", std::process::id()).expect("the pid is announced");
    output.flush().expect("the pid is announced");
}
