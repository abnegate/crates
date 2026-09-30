use tokio::process::Child;
use tokio::runtime::Handle;

use crate::tool::process::Group;

/// A stdio server's process, which leads a process group of its own.
///
/// Nothing reaps the server while this holds it, even once it has exited by
/// itself: an unreaped server keeps its pid, and so its group's id, from
/// being given to anyone else, so however long a session outlives its server,
/// the kill its drop sends can reach only the server's own group. Dropping
/// this sends that group its first kill at once, so a runtime shutting down
/// before it gets to the rest cannot leave the group running, and only then
/// reaps the server, on the runtime when there is one, where the kill is left
/// to finish repeating itself first.
#[derive(Debug)]
pub(super) struct ServerProcess {
    server: Option<Child>,
    group: Group,
}

impl ServerProcess {
    /// Hold `server`, which has to have been spawned with `process_group(0)`.
    pub(super) fn new(server: Child) -> Self {
        let group = Group::led_by(server.id());
        Self {
            server: Some(server),
            group,
        }
    }
}

impl Drop for ServerProcess {
    fn drop(&mut self) {
        let Some(mut server) = self.server.take() else {
            return;
        };
        let mut group = std::mem::replace(&mut self.group, Group::led_by(None));
        group.kill_now();
        match Handle::try_current() {
            Ok(runtime) => {
                runtime.spawn(async move {
                    group.kill_until_gone().await;
                    let _ = server.wait().await;
                });
            }
            Err(_) => group.kill(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::process::Stdio;
    use std::time::Duration;
    use std::time::Instant;

    use tokio::process::Command;
    use tokio::runtime::Builder;

    use super::ServerProcess;
    use crate::test_support::PATIENCE;

    /// Whether `pid` still runs: neither reaped nor a zombie.
    fn running(pid: i32) -> bool {
        let output = std::process::Command::new("ps")
            .args(["-o", "stat=", "-p", &pid.to_string()])
            .output()
            .expect("a process listing");
        let state = String::from_utf8_lossy(&output.stdout).trim().to_string();
        !state.is_empty() && !state.starts_with('Z')
    }

    fn eventually(condition: impl Fn() -> bool) -> bool {
        let started = Instant::now();
        while !condition() {
            if started.elapsed() > PATIENCE {
                return false;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        true
    }

    /// The pid `path` names once the server has written it.
    fn written(path: &Path) -> i32 {
        let read = || std::fs::read_to_string(path).unwrap_or_default();
        assert!(
            eventually(|| read().ends_with('\n')),
            "the server never wrote its helper's pid"
        );
        read().trim().parse().expect("a pid")
    }

    /// A drop left the whole kill to a task on the runtime, so a runtime that
    /// never ran again, as one shutting down or leaked at exit need not, left
    /// the server's helpers running. The first kill is now sent by the drop
    /// itself; only the reap waits for the runtime.
    #[test]
    fn a_server_dropped_on_a_runtime_that_never_runs_again_leaves_no_helper() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let path = directory.path().join("helper");
        let runtime = Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a runtime");
        let process = runtime.block_on(async {
            let server = Command::new("sh")
                .args(["-c", "sleep 300 & echo $! > \"$1\"; wait", "sh"])
                .arg(&path)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .process_group(0)
                .spawn()
                .expect("the server starts");
            ServerProcess::new(server)
        });
        let helper = written(&path);

        runtime.block_on(async move { drop(process) });
        std::mem::forget(runtime);

        let gone = eventually(|| !running(helper));
        if !gone {
            let _ = nix::sys::signal::kill(
                nix::unistd::Pid::from_raw(helper),
                nix::sys::signal::Signal::SIGKILL,
            );
        }
        assert!(
            gone,
            "the dropped server's helper {helper} is still running"
        );
    }
}
