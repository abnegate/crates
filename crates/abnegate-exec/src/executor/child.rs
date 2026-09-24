//! Re-running a test in a child test process with an environment of its
//! own, so a test can shape the executor's environment without touching
//! this process's.

use std::ffi::OsStr;

use tokio::process::Command;

use super::sandbox::REQUIRE_CONFINEMENT;

/// Set to the name of the test a child process runs.
const CHILD: &str = "ABNEGATE_EXEC_TEST_CHILD";

/// Re-run the test `name` in a child test process whose environment is
/// `PATH` and [`REQUIRE_CONFINEMENT`] plus `environment`, which also starts
/// with no sandbox verdict cached. Returns whether this call was the parent,
/// which has nothing left to do once the child passes.
pub(crate) async fn delegated(name: &str, environment: &[(&str, &str)]) -> bool {
    let environment: Vec<(&str, &OsStr)> = environment
        .iter()
        .map(|&(variable, value)| (variable, OsStr::new(value)))
        .collect();
    delegated_os(name, &environment).await
}

/// [`delegated`], with values that need not be UTF-8.
pub(crate) async fn delegated_os(name: &str, environment: &[(&str, &OsStr)]) -> bool {
    if std::env::var(CHILD).as_deref() == Ok(name) {
        return false;
    }
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", name, "--nocapture"])
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env(CHILD, name)
        .envs(std::env::var_os(REQUIRE_CONFINEMENT).map(|value| (REQUIRE_CONFINEMENT, value)))
        .envs(environment.iter().copied())
        .output()
        .await
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "{stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        stdout.contains("1 passed"),
        "the child ran no test, so it proved nothing\n{stdout}"
    );
    true
}
