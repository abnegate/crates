use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use nix::errno::Errno;
use tempfile::TempDir;

use super::exits_within;

/// The bytes of the sign-in fixture `name`, under `tests/fixtures/login`.
macro_rules! fixture {
    ($name:literal) => {
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/login/",
            $name
        ))
    };
}

pub(crate) use fixture;

pub(crate) const PROMPT: &[u8] = fixture!("codex-login-device-auth.stdout");
pub(crate) const SUCCESS_PROMPT: &[u8] =
    fixture!("fake-issuer/codex-login-device-auth-success.stdout");
pub(crate) const SUCCESS: &[u8] = fixture!("fake-issuer/codex-login-device-auth-success.stderr");
pub(crate) const REFUSED: &[u8] =
    fixture!("fake-issuer/codex-login-device-auth-refused-403.stderr");
pub(crate) const UNSUPPORTED: &[u8] =
    fixture!("fake-issuer/codex-login-device-auth-refused-404.stderr");
pub(crate) const THROTTLED: &[u8] =
    fixture!("fake-issuer/codex-login-device-auth-refused-429.stderr");
pub(crate) const POLL_FAILED_PROMPT: &[u8] =
    fixture!("fake-issuer/codex-login-device-auth-poll-failed-500.stdout");
pub(crate) const POLL_FAILED: &[u8] =
    fixture!("fake-issuer/codex-login-device-auth-poll-failed-500.stderr");
pub(crate) const EXCHANGE_FAILED_PROMPT: &[u8] =
    fixture!("fake-issuer/codex-login-device-auth-exchange-failed-400.stdout");
pub(crate) const EXCHANGE_FAILED: &[u8] =
    fixture!("fake-issuer/codex-login-device-auth-exchange-failed-400.stderr");
pub(crate) const RELOGIN_REFUSED: &[u8] =
    fixture!("fake-issuer/codex-login-device-auth-relogin-refused.stderr");
pub(crate) const RELOGIN_ABANDONED_PROMPT: &[u8] =
    fixture!("fake-issuer/codex-login-device-auth-relogin-abandoned.stdout");
pub(crate) const NO_COLOR_PROMPT: &[u8] =
    fixture!("fake-issuer/codex-login-device-auth-no-color.stdout");
pub(crate) const TERM_DUMB_PROMPT: &[u8] =
    fixture!("fake-issuer/codex-login-device-auth-term-dumb.stdout");

const PATH: &str = "/usr/bin:/bin";
const SHELL_MAINTAINED: &[&str] = &["PWD", "OLDPWD", "SHLVL", "_"];
const ATTEMPTS: usize = 100;
const PAUSE: Duration = Duration::from_millis(50);

/// All a fake CLI needs to run its script.
pub(crate) fn environment() -> BTreeMap<String, String> {
    BTreeMap::from([("PATH".to_string(), PATH.to_string())])
}

/// Writes `bytes` beside the fake, so its script can replay them byte for
/// byte.
pub(crate) fn capture(directory: &TempDir, name: &str, bytes: &[u8]) -> String {
    let path = directory.path().join(name);
    std::fs::write(&path, bytes).expect("the capture to be written");
    path.display().to_string()
}

/// A stand-in CLI, so no test needs a real codex or claude installed.
///
/// It runs `script` only when called with exactly `arguments`. Anything else
/// exits at once, which is what makes the run in [`wait_until_executable`]
/// harmless.
pub(crate) fn fake(directory: &TempDir, arguments: &str, script: &str) -> PathBuf {
    let path = directory.path().join("cli");
    let body = format!(
        "#!/bin/sh\n[ \"$*\" = '{arguments}' ] || {{ echo \"unexpected arguments: $*\" >&2; exit 64; }}\n{script}\n"
    );
    std::fs::write(&path, body).expect("the fake CLI to be written");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
        .expect("the fake CLI to be executable");
    wait_until_executable(&path);
    path
}

/// Runs the fake once without arguments, so it exits at once, before a
/// test's deadlines start.
///
/// Linux refuses to exec a file any process still holds open for writing. A
/// sibling test forking between its own open and exec inherits this one's
/// descriptor for that window, so a freshly written script can hit `ETXTBSY`
/// under a parallel run. macOS assesses a new executable on its first run,
/// which can take seconds: a run killed at once leaves the next run as slow,
/// and a run to completion does not.
fn wait_until_executable(path: &Path) {
    for _ in 0..ATTEMPTS {
        match std::process::Command::new(path)
            .env_clear()
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
        {
            Err(error) if error.raw_os_error() == Some(Errno::ETXTBSY as i32) => {
                std::thread::sleep(PAUSE);
            }
            _ => return,
        }
    }
}

/// The environment a fake saved with `env`, less the variables `/bin/sh`
/// sets for itself.
pub(crate) fn recorded(path: &Path) -> BTreeMap<String, String> {
    std::fs::read_to_string(path)
        .expect("the fake to have recorded its environment")
        .lines()
        .filter_map(|line| line.split_once('='))
        .filter(|(name, _)| !SHELL_MAINTAINED.contains(name))
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect()
}

/// The process id a fake saved with `echo $$` or `echo $!`.
pub(crate) fn process(path: &Path) -> u32 {
    std::fs::read_to_string(path)
        .expect("the fake to have recorded a process id")
        .trim()
        .parse()
        .expect("a process id")
}

/// Whether `process` stops running in good time. A killed grandchild
/// lingers as a zombie until whoever inherited it reaps it, and a zombie
/// runs nothing.
pub(crate) fn ended(process: u32) -> bool {
    exits_within(process)
}
