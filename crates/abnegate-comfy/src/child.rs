//! Re-running a test in a child test process with an environment of its own,
//! so a test can give the crate's host a variable without touching this
//! process's environment.

use tokio::process::Command;

/// Set to the name of the test a child process runs.
const CHILD: &str = "ABNEGATE_COMFY_TEST_CHILD";

/// Re-run the test `name` in a child test process whose environment is `PATH`
/// plus `environment`. Returns whether this call was the parent, which has
/// nothing left to do once the child passes.
pub(crate) async fn delegated(name: &str, environment: &[(&str, &str)]) -> bool {
    if std::env::var(CHILD).as_deref() == Ok(name) {
        return false;
    }
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", name, "--nocapture"])
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env(CHILD, name)
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
