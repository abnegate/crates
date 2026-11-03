use std::os::unix::process::CommandExt;
use std::process::Child;
use std::process::Command;
use std::process::Stdio;
use std::sync::Mutex;
use std::sync::PoisonError;
use std::time::Duration;
use std::time::Instant;

use nix::fcntl::OFlag;
use nix::pty::PtyMaster;
use nix::pty::grantpt;
use nix::pty::posix_openpt;
use nix::pty::ptsname;
use nix::pty::unlockpt;

use super::process_group::ProcessGroup;

/// How long the member is given to reach its exit, however loaded the
/// machine.
const PATIENCE: Duration = Duration::from_secs(60);

/// Held while `ptsname` fills its static buffer.
static NAMING: Mutex<()> = Mutex::new(());

/// A member of a process group held part way through its exit until it is
/// dropped: macOS has it wait, as it closes its terminal for the last time,
/// for the output it left there to be read, and nothing reads it. Through
/// that wait the kernel no longer signals it, nor counts it among the
/// group's members when a signal to the group finds no one to deliver to,
/// though it is not yet a zombie.
///
/// Only the member opens the terminal's other end, so its close is the last
/// one: a child that another test spawned meanwhile inherits neither end.
pub(crate) struct HeldExit {
    member: Child,
    terminal: Option<PtyMaster>,
}

impl HeldExit {
    /// Start a member of `group` that writes to a terminal and exits, and
    /// return once it is held in its exit.
    pub(crate) fn join(group: &ProcessGroup) -> Self {
        let (terminal, name) = terminal();
        let member = Command::new("sh")
            .args(["-c", r#"exec > "$0"; echo unread"#, &name])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(group.pgid())
            .spawn()
            .expect("the member starts");
        let held = Self {
            member,
            terminal: Some(terminal),
        };
        let started = Instant::now();
        while !held.is_exiting() {
            assert!(
                started.elapsed() < PATIENCE,
                "the member was never held in its exit"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        held
    }

    pub(crate) fn pid(&self) -> i32 {
        i32::try_from(self.member.id()).expect("a pid fits an i32")
    }

    /// Whether `ps` shows the member trying to exit.
    fn is_exiting(&self) -> bool {
        let output = Command::new("ps")
            .args(["-o", "stat=", "-p", &self.member.id().to_string()])
            .output()
            .expect("a process listing");
        String::from_utf8_lossy(&output.stdout).contains('E')
    }
}

impl Drop for HeldExit {
    fn drop(&mut self) {
        drop(self.terminal.take());
        let _ = self.member.wait();
    }
}

/// A terminal's controlling end, closed on exec, and the path of its other
/// end.
fn terminal() -> (PtyMaster, String) {
    let terminal =
        posix_openpt(OFlag::O_RDWR | OFlag::O_NOCTTY | OFlag::O_CLOEXEC).expect("a terminal opens");
    grantpt(&terminal).expect("the terminal is granted");
    unlockpt(&terminal).expect("the terminal is unlocked");
    let _naming = NAMING.lock().unwrap_or_else(PoisonError::into_inner);
    // SAFETY: nothing else in this process calls ptsname, and NAMING keeps
    // these calls to one at a time, so none overwrites the static buffer
    // before the name is copied out of it.
    let name = unsafe { ptsname(&terminal) }.expect("the terminal is named");
    (terminal, name)
}
