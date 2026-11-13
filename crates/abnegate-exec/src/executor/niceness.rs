use std::io;

use nix::libc;
use tokio::process::Command;

use crate::error::NicenessError;

/// How little of a contended CPU a process claims beside its peers: the
/// scheduler's nice value, from 0, an equal share, to 19, the least.
///
/// A process may always lower its own priority but needs privilege to raise
/// it, so no negative value is offered. Every process inherits its parent's
/// nice value, so one [applied](Niceness::apply) to the command a run starts
/// holds for the whole tree it starts in turn. Where no IO priority was set,
/// Linux derives one from the nice value, so a lowered process yields the
/// disk too under a scheduler that honours IO priorities.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Niceness(u8);

impl Niceness {
    /// An equal share of the CPU, as a process gets unless told otherwise.
    pub const DEFAULT: Self = Self(0);

    /// The least share of the CPU: such a process runs when nothing at a
    /// higher priority wants the CPU, and barely otherwise.
    pub const LOWEST: Self = Self(19);

    /// The nice value.
    pub const fn get(self) -> u8 {
        self.0
    }

    /// Start `command` at this nice value. A spawn fails rather than run the
    /// process at another one, as it does when the host itself already runs
    /// at a higher nice value without the privilege to lower it.
    pub fn apply(self, command: &mut Command) {
        let niceness = libc::c_int::from(self.0);
        // Runs between fork and exec, where only async-signal-safe calls are
        // permitted: `setpriority` is, and neither it nor reading `errno`
        // allocates. The child has a single thread, so on Linux, where the
        // nice value is a thread's, it is the whole process's.
        #[allow(unsafe_code)]
        unsafe {
            command.pre_exec(move || {
                if libc::setpriority(libc::PRIO_PROCESS, 0, niceness) == -1 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
}

impl Default for Niceness {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl TryFrom<u8> for Niceness {
    type Error = NicenessError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        if value > Self::LOWEST.0 {
            return Err(NicenessError::OutOfRange(value));
        }
        Ok(Self(value))
    }
}

#[cfg(test)]
mod tests {
    use std::process::Stdio;

    use super::*;

    /// The nice value the shell `command` starts reports for itself.
    async fn reported(mut command: Command) -> String {
        let output = command
            .args(["-c", "ps -o nice= -p $$"])
            .stdout(Stdio::piped())
            .output()
            .await
            .expect("the shell's report of its own nice value");
        assert!(output.status.success(), "ps failed: {output:?}");
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    #[tokio::test]
    async fn a_lowered_command_runs_at_its_nice_value() {
        let mut command = Command::new("sh");
        Niceness::LOWEST.apply(&mut command);

        assert_eq!(reported(command).await, "19");
    }

    #[tokio::test]
    async fn the_processes_a_lowered_command_starts_inherit_its_nice_value() {
        let mut command = Command::new("sh");
        command.args(["-c", "sh -c 'ps -o nice= -p $$'"]);
        Niceness::LOWEST.apply(&mut command);

        let output = command
            .stdout(Stdio::piped())
            .output()
            .await
            .expect("the grandchild's report of its own nice value");

        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "19");
    }

    #[tokio::test]
    async fn the_hosts_own_nice_value_is_one_a_command_can_start_at() {
        let host = reported(Command::new("sh")).await;
        let mut command = Command::new("sh");
        let value: u8 = host.parse().expect("a numeric nice value");
        Niceness::try_from(value)
            .expect("the host's own nice value")
            .apply(&mut command);

        assert_eq!(reported(command).await, host);
    }

    #[test]
    fn every_value_from_an_equal_share_to_the_least_is_a_niceness() {
        for value in Niceness::DEFAULT.get()..=Niceness::LOWEST.get() {
            assert_eq!(
                Niceness::try_from(value).map(Niceness::get).ok(),
                Some(value)
            );
        }
    }

    #[test]
    fn a_value_past_the_least_share_is_refused() {
        let error = Niceness::try_from(20).expect_err("20 is past the lowest priority");

        assert!(matches!(error, NicenessError::OutOfRange(20)));
    }

    #[test]
    fn a_default_niceness_takes_an_equal_share() {
        assert_eq!(Niceness::default(), Niceness::DEFAULT);
        assert_eq!(Niceness::DEFAULT.get(), 0);
    }
}
