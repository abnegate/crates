use std::os::unix::process::CommandExt;
use std::os::unix::process::ExitStatusExt;
use std::process::Child;
use std::process::Command;
use std::process::Stdio;

use super::process_group::ProcessGroup;

/// A real `sleep` leading its own process group, for tests that signal one.
/// Dropping it kills and reaps the child.
pub(crate) struct Sleeper {
    child: Child,
}

impl Sleeper {
    pub(crate) fn start() -> Self {
        let child = Command::new("sleep")
            .arg("30")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()
            .expect("sleep starts");
        Self { child }
    }

    pub(crate) fn pid(&self) -> u32 {
        self.child.id()
    }

    pub(crate) fn group(&self) -> ProcessGroup {
        ProcessGroup::try_from(self.pid()).expect("a child leading its own group")
    }

    /// Reap the child and return the signal that ended it, if one did.
    pub(crate) fn wait(&mut self) -> Option<i32> {
        self.child.wait().expect("the child is reaped").signal()
    }
}

impl Drop for Sleeper {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
