use std::process::Output;

use tokio::process::Command;

/// Set in a test's own child process, naming the test the child runs, so a
/// test that needs this process's environment shaped can re-run itself.
const CHILD_TEST: &str = "ABNEGATE_AGENT_CLI_CHILD_TEST";

/// Re-run the test `name` in a child test process whose environment is
/// `PATH` and `TMPDIR` plus `environment`, unless this is that child.
/// Returns whether this call was the parent, which has nothing left to do
/// once the child passed.
pub(crate) async fn delegated(name: &str, environment: &[(&str, &str)]) -> bool {
    if std::env::var(CHILD_TEST).as_deref() == Ok(name) {
        return false;
    }
    let inherited = ["PATH", "TMPDIR"]
        .into_iter()
        .filter_map(|variable| std::env::var_os(variable).map(|value| (variable, value)));
    let output = Command::new(std::env::current_exe().expect("the test binary"))
        .args(["--exact", name, "--nocapture"])
        .env_clear()
        .envs(inherited)
        .env(CHILD_TEST, name)
        .envs(environment.iter().copied())
        .output()
        .await
        .expect("the child test");
    assert_passed(&output);
    true
}

/// Fail unless the re-run of one test that produced `output` passed and ran
/// that test at all: a name that matches no test runs nothing and still
/// exits zero, proving nothing.
pub(crate) fn assert_passed(output: &Output) {
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
}
