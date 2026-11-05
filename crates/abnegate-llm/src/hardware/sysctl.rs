/// macOS's own `sysctl`, named by its path so that no `sysctl` found earlier
/// on `PATH`, such as one in the working directory that an empty or relative
/// `PATH` entry names, runs in its place.
const SYSCTL: &str = "/usr/sbin/sysctl";

/// What `sysctl` prints for `key`, trimmed, or `None` when it cannot be run.
/// It runs with an empty environment.
pub(crate) fn read(key: &str) -> Option<String> {
    let output = std::process::Command::new(SYSCTL)
        .args(["-n", key])
        .env_clear()
        .output()
        .ok()?;
    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}
