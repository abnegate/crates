use std::io;
use std::str::FromStr;

use nix::errno::Errno;
use nix::libc;
use tokio::process::Command;

use crate::error::NicenessError;

/// How little of a contended CPU a process claims beside its peers: the
/// scheduler's nice value, from 0, the usual priority, to 19, the lowest.
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
    /// The priority a process starts at unless it, or an ancestor, was niced.
    pub const DEFAULT: Self = Self(0);

    /// The lowest priority: such a process runs when nothing at a higher
    /// priority wants the CPU, and barely otherwise.
    pub const LOWEST: Self = Self(19);

    /// The nice value.
    pub const fn get(self) -> u8 {
        self.0
    }

    /// Start `command` at this nice value, or at the host's own when the host
    /// already runs at a lower priority, since raising it back needs
    /// privilege. A command is never started at a higher priority than its
    /// host's.
    pub fn apply(self, command: &mut Command) {
        let niceness = libc::c_int::from(self.0);
        // Runs between fork and exec, so it allocates nothing: it clears and
        // reads `errno` and calls `getpriority` and `setpriority`, bare
        // syscall wrappers on every supported libc, though POSIX does not
        // list them as async-signal-safe. The child has a single thread, so
        // on Linux, where the nice value is a thread's, it is the whole
        // process's.
        #[allow(unsafe_code)]
        unsafe {
            command.pre_exec(move || {
                Errno::clear();
                let current = libc::getpriority(libc::PRIO_PROCESS, 0);
                if current == -1 && Errno::last_raw() != 0 {
                    return Err(io::Error::last_os_error());
                }
                if libc::setpriority(libc::PRIO_PROCESS, 0, niceness.max(current)) == -1 {
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

impl FromStr for Niceness {
    type Err = NicenessError;

    /// A whole number from 0 to 19, around which whitespace is ignored.
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let trimmed = value.trim();
        trimmed
            .parse::<u8>()
            .map_err(|_| NicenessError::Unparsable(trimmed.to_string()))
            .and_then(Self::try_from)
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

    fn niceness(value: u8) -> Niceness {
        Niceness::try_from(value).expect("a nice value")
    }

    #[tokio::test]
    async fn a_lowered_command_runs_at_its_nice_value() {
        let host = reported(Command::new("sh")).await;
        assert_ne!(host, "19", "the host already runs at the lowest priority");
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

    /// The first value stands for a host already niced to 12, which cannot
    /// raise its children back to 5.
    #[tokio::test]
    async fn a_command_never_starts_above_its_hosts_priority() {
        let mut command = Command::new("sh");
        niceness(12).apply(&mut command);
        niceness(5).apply(&mut command);

        assert_eq!(reported(command).await, "12");
    }

    #[test]
    fn every_value_from_the_usual_priority_to_the_lowest_is_a_niceness() {
        for value in Niceness::DEFAULT.get()..=Niceness::LOWEST.get() {
            assert_eq!(
                Niceness::try_from(value).map(Niceness::get).ok(),
                Some(value)
            );
        }
    }

    #[test]
    fn a_value_past_the_lowest_priority_is_refused() {
        let error = Niceness::try_from(20).expect_err("20 is past the lowest priority");

        assert!(matches!(error, NicenessError::OutOfRange(20)));
    }

    #[test]
    fn a_default_niceness_is_the_usual_priority() {
        assert_eq!(Niceness::default(), Niceness::DEFAULT);
        assert_eq!(Niceness::DEFAULT.get(), 0);
    }

    #[test]
    fn a_whole_number_from_0_to_19_parses() {
        for (text, value) in [("0", 0), (" 10 ", 10), ("19", 19)] {
            assert_eq!(text.parse::<Niceness>().map(Niceness::get), Ok(value));
        }
    }

    #[test]
    fn anything_else_is_refused_by_name() {
        assert_eq!("20".parse::<Niceness>(), Err(NicenessError::OutOfRange(20)));
        for text in ["-1", "low", "1.5", "256", ""] {
            assert_eq!(
                text.parse::<Niceness>(),
                Err(NicenessError::Unparsable(text.to_string())),
                "{text:?}"
            );
        }
    }
}
