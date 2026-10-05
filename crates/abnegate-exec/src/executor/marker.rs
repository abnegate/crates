use std::time::Duration;
use std::time::Instant;

use nix::sys::signal::Signal;
use tokio::process::Command;
use uuid::Uuid;

use crate::error::MarkerError;

#[cfg(target_os = "linux")]
#[allow(unsafe_code)]
mod linux;
#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod macos;
#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
mod straggler;
mod sweep;
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
mod unsupported;

#[cfg(target_os = "linux")]
use linux::Scanner;
#[cfg(target_os = "macos")]
use macos::Scanner;
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
use unsupported::Scanner;

use sweep::Sweep;

const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Tags a command's environment so the processes it starts can be found and
/// stopped after they leave its process group.
///
/// Killing a run's process group misses whatever moved to a session or group
/// of its own, such as a daemon a tool started. Every process inherits its
/// parent's environment, so a marker [applied](Marker::apply) to the command
/// a run starts is carried by everything the run starts in turn, wherever it
/// went. [`find`](Marker::find), [`terminate`](Marker::terminate) and
/// [`kill`](Marker::kill) reach the processes whose own environment holds the
/// marker's exact `variable=value` entry.
///
/// A sweep only considers processes of the caller's effective user and never
/// the caller itself, and a zombie never matches. A process that clears the
/// variable before it starts another hides that one from the sweep. On macOS,
/// Apple's platform binaries, such as `/bin/sleep`, hide their environment
/// from other processes, so they are never found. On hosts other than Linux
/// and macOS a sweep finds nothing.
///
/// On Linux a process is checked for the marker and signalled through a
/// pidfd, so a signal never reaches a process that took over a marked
/// process's pid. macOS has no pidfd: there a process is checked and then
/// signalled by pid, so a process that took over the pid of a marked one
/// that exited between the two can receive the signal.
///
/// Sweeps read every process's environment and block the calling thread;
/// call them from [`tokio::task::spawn_blocking`] in async code.
///
/// ```
/// use std::time::Duration;
///
/// use abnegate_exec::Marker;
/// use abnegate_exec::MarkerError;
/// use tokio::process::Command;
///
/// # fn run() -> Result<(), MarkerError> {
/// let marker = Marker::new("MY_APPLICATION_RUN")?;
/// let mut command = Command::new("my-agent");
/// marker.apply(&mut command);
///
/// let stopped = marker.terminate(Duration::from_secs(5))?;
/// println!("stopped {} processes the run left behind", stopped.len());
/// # Ok(())
/// # }
/// # run().unwrap();
/// ```
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Marker {
    variable: String,
    value: String,
}

impl Marker {
    /// A marker that sets the environment variable `variable` to a value no
    /// other marker has.
    ///
    /// # Errors
    ///
    /// [`MarkerError::InvalidVariable`] when `variable` is empty or holds `=`
    /// or a NUL byte, none of which an environment entry can carry as the
    /// variable's name.
    pub fn new(variable: impl Into<String>) -> Result<Self, MarkerError> {
        let variable = variable.into();
        if variable.is_empty() || variable.contains(['=', '\0']) {
            return Err(MarkerError::InvalidVariable(variable));
        }
        Ok(Self {
            variable,
            value: Uuid::new_v4().hyphenated().to_string(),
        })
    }

    /// The name of the environment variable that carries the marker.
    pub fn variable(&self) -> &str {
        &self.variable
    }

    /// The value the marker sets its variable to.
    pub fn value(&self) -> &str {
        &self.value
    }

    /// Set the marker's variable in `command`'s environment, so the process
    /// it starts, and every process that one starts, carries the marker.
    pub fn apply(&self, command: &mut Command) {
        command.env(&self.variable, &self.value);
    }

    /// The processes that carry the marker.
    ///
    /// # Errors
    ///
    /// [`MarkerError::ListFailed`] when the host's processes cannot be listed.
    pub fn find(&self) -> Result<Vec<u32>, MarkerError> {
        Scanner::new()
            .and_then(|mut scanner| scanner.find(self.entry().as_bytes()))
            .map_err(MarkerError::ListFailed)
    }

    /// Send SIGTERM to every process that carries the marker, give them up to
    /// `grace` to exit, then [`kill`](Marker::kill) whatever is left. Returns
    /// the processes it signalled, once none is left or the grace has run out
    /// and the kill has been sent.
    ///
    /// # Errors
    ///
    /// The first failure among its sweeps, once every sweep has run:
    /// [`MarkerError::ListFailed`] when the host's processes cannot be listed,
    /// and [`MarkerError::SignalFailed`] when a process that carries the
    /// marker refuses a signal.
    pub fn terminate(&self, grace: Duration) -> Result<Vec<u32>, MarkerError> {
        let mut scanner = Scanner::new().map_err(MarkerError::ListFailed)?;
        let entry = self.entry();
        let mut sweep = Sweep::default();
        send(&mut scanner, entry.as_bytes(), Signal::SIGTERM, &mut sweep);
        let start = Instant::now();
        while let Some(remaining) = grace.checked_sub(start.elapsed())
            && scanner
                .find(entry.as_bytes())
                .is_ok_and(|processes| !processes.is_empty())
        {
            std::thread::sleep(POLL_INTERVAL.min(remaining));
        }
        kill(&mut scanner, entry.as_bytes(), &mut sweep);
        sweep.finish()
    }

    /// Send SIGKILL to every process that carries the marker, and again to
    /// catch a child one of them forked as the first one landed. Returns the
    /// processes it signalled.
    ///
    /// # Errors
    ///
    /// As for [`terminate`](Marker::terminate).
    pub fn kill(&self) -> Result<Vec<u32>, MarkerError> {
        let mut scanner = Scanner::new().map_err(MarkerError::ListFailed)?;
        let mut sweep = Sweep::default();
        kill(&mut scanner, self.entry().as_bytes(), &mut sweep);
        sweep.finish()
    }

    fn entry(&self) -> String {
        format!("{}={}", self.variable, self.value)
    }
}

fn kill(scanner: &mut Scanner, entry: &[u8], sweep: &mut Sweep) {
    send(scanner, entry, Signal::SIGKILL, sweep);
    send(scanner, entry, Signal::SIGKILL, sweep);
}

fn send(scanner: &mut Scanner, entry: &[u8], signal: Signal, sweep: &mut Sweep) {
    match scanner.find(entry) {
        Ok(processes) => {
            for pid in processes {
                sweep.record(pid, scanner.deliver(pid, entry, signal));
            }
        }
        Err(error) => sweep.fail(MarkerError::ListFailed(error)),
    }
}

#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
mod tests {
    use std::os::unix::process::CommandExt;
    use std::time::Duration;
    use std::time::Instant;

    use super::straggler::Behaviour;
    use super::straggler::CLEANED;
    use super::straggler::EXIT_DEADLINE;
    use super::straggler::Straggler;
    use super::straggler::VARIABLE;
    use super::straggler::exits_within;
    use super::straggler::stop_if_running;
    use super::*;

    const GRACE: Duration = Duration::from_millis(500);

    /// How long a test waits after a sweep before it checks that a process
    /// the sweep must spare is still running.
    const SETTLE: Duration = Duration::from_millis(200);

    fn marker() -> Marker {
        Marker::new(VARIABLE).unwrap()
    }

    #[test]
    fn a_marker_carries_the_callers_variable_and_a_value_of_its_own() {
        let first = marker();
        let second = marker();

        assert_eq!(first.variable(), VARIABLE);
        assert!(Uuid::parse_str(first.value()).is_ok());
        assert_ne!(first.value(), second.value());
        assert_ne!(first, second);
    }

    #[test]
    fn a_variable_name_no_environment_entry_can_carry_is_refused() {
        for variable in ["", "RUN=", "=RUN", "R=UN", "RUN\0", "R\0UN"] {
            assert!(
                matches!(
                    Marker::new(variable),
                    Err(MarkerError::InvalidVariable(refused)) if refused == variable
                ),
                "{variable:?} must not name a marker's variable"
            );
        }
    }

    #[test]
    fn find_reports_a_marked_process_whose_program_name_is_empty() {
        let marker = marker();
        let mut command = Straggler::marked(Behaviour::Stays, &marker);
        command.arg0("");
        let straggler = Straggler::start(&mut command, 1);

        assert_eq!(marker.find().unwrap(), vec![straggler.pid()]);
    }

    #[test]
    fn find_reports_a_marked_process_and_nothing_else() {
        let marker = marker();
        let marked = Straggler::start(&mut Straggler::marked(Behaviour::Stays, &marker), 1);
        let _unmarked = Straggler::start(&mut Straggler::command(Behaviour::Stays), 1);

        assert_eq!(marker.find().unwrap(), vec![marked.pid()]);
    }

    #[test]
    fn apply_marks_the_process_a_command_starts() {
        let marker = marker();
        let mut command = Command::from(Straggler::command(Behaviour::Stays));

        marker.apply(&mut command);
        let straggler = Straggler::start(command.as_std_mut(), 1);

        assert_eq!(marker.find().unwrap(), vec![straggler.pid()]);
    }

    #[test]
    fn a_sweep_with_nothing_marked_signals_nothing_and_does_not_wait() {
        let marker = marker();

        let start = Instant::now();
        let terminated = marker.terminate(EXIT_DEADLINE).unwrap();
        let elapsed = start.elapsed();

        assert!(terminated.is_empty(), "terminate signalled {terminated:?}");
        assert!(marker.kill().unwrap().is_empty());
        assert!(
            elapsed < EXIT_DEADLINE / 2,
            "terminate took {elapsed:?} with nothing to wait for"
        );
    }

    #[test]
    fn kill_ends_a_marked_process_in_a_session_of_its_own() {
        let marker = marker();
        let escaped = Straggler::start(&mut Straggler::marked(Behaviour::Escapes, &marker), 1);
        let pid = escaped.pid();

        let killed = marker.kill().unwrap();

        assert!(
            escaped.stops(),
            "process {pid} carries the marker in a session of its own but outlived kill"
        );
        assert_eq!(killed, vec![pid]);
    }

    #[test]
    fn kill_ends_a_marked_process_in_the_callers_own_session() {
        let marker = marker();
        let straggler = Straggler::start(&mut Straggler::marked(Behaviour::Stays, &marker), 1);

        marker.kill().unwrap();

        assert!(
            straggler.stops(),
            "a marked process in the caller's session outlived kill"
        );
    }

    #[test]
    fn kill_spares_near_misses_of_the_marker() {
        let marker = marker();
        let value = marker.value();
        let entry = format!("{VARIABLE}={value}");
        let mut extended = Straggler::command(Behaviour::Stays);
        extended.env(VARIABLE, format!("{value}x"));
        let mut truncated = Straggler::command(Behaviour::Stays);
        truncated.env(VARIABLE, &value[..value.len() - 1]);
        let mut prefixed = Straggler::command(Behaviour::Stays);
        prefixed.env(format!("X{VARIABLE}"), value);
        let mut argument = Straggler::command(Behaviour::Stays);
        argument.arg(&entry);
        let mut near_misses = [
            Straggler::command(Behaviour::Stays),
            extended,
            truncated,
            prefixed,
            argument,
            Straggler::marked(Behaviour::Stays, &Marker::new(VARIABLE).unwrap()),
        ]
        .map(|mut command| Straggler::start(&mut command, 1));
        let exact = Straggler::start(&mut Straggler::marked(Behaviour::Stays, &marker), 1);

        marker.kill().unwrap();

        let stopped = exact.stops();
        std::thread::sleep(SETTLE);
        let killed: Vec<u32> = near_misses
            .iter_mut()
            .filter_map(|straggler| (!straggler.is_running()).then(|| straggler.pid()))
            .collect();
        assert!(
            stopped,
            "the process carrying the exact entry outlived kill"
        );
        assert!(
            killed.is_empty(),
            "processes {killed:?} without the exact entry in their environment were killed"
        );
    }

    #[test]
    fn kill_spares_unmarked_processes() {
        let marker = marker();
        let mut unmarked = Straggler::start(&mut Straggler::command(Behaviour::Stays), 1);
        let session = Straggler::start(&mut Straggler::marked(Behaviour::Forks, &marker), 2);
        let sibling = session.pids()[1];

        marker.kill().unwrap();

        let stopped = session.stops();
        std::thread::sleep(SETTLE);
        let unmarked_running = unmarked.is_running();
        let sibling_running = stop_if_running(sibling);
        assert!(stopped, "the marked process outlived kill");
        assert!(
            unmarked_running,
            "unmarked process {} in the caller's session was killed",
            unmarked.pid()
        );
        assert!(
            sibling_running,
            "unmarked process {sibling} in a marked process's session was killed"
        );
    }

    #[test]
    fn terminate_does_not_wait_out_its_grace_on_a_zombie() {
        let marker = marker();
        let mut straggler = Straggler::start(&mut Straggler::marked(Behaviour::Stays, &marker), 1);
        let pid = straggler.pid();
        straggler.kill();
        let exited = exits_within(pid);

        let start = Instant::now();
        let terminated = marker.terminate(EXIT_DEADLINE).unwrap();
        let elapsed = start.elapsed();

        assert!(exited, "process {pid} outlived SIGKILL");
        assert!(
            elapsed < EXIT_DEADLINE / 2,
            "terminate took {elapsed:?} waiting on zombie {pid}, which nothing may reap"
        );
        assert!(terminated.is_empty(), "terminate signalled {terminated:?}");
    }

    #[test]
    fn terminate_lets_a_process_clean_up_before_it_stops() {
        let directory = tempfile::tempdir().unwrap();
        let cleaned = directory.path().join("cleaned");
        let marker = marker();
        let mut command = Straggler::marked(Behaviour::CleansUp, &marker);
        command.env(CLEANED, &cleaned);
        let straggler = Straggler::start(&mut command, 1);
        let pid = straggler.pid();

        let start = Instant::now();
        let terminated = marker.terminate(EXIT_DEADLINE).unwrap();
        let elapsed = start.elapsed();

        assert!(straggler.stops(), "the process outlived terminate");
        assert!(
            cleaned.exists(),
            "the process was stopped before it could clean up on SIGTERM"
        );
        assert!(
            elapsed < EXIT_DEADLINE / 2,
            "terminate took {elapsed:?} although the process stopped on SIGTERM"
        );
        assert_eq!(terminated, vec![pid]);
    }

    #[test]
    fn terminate_kills_a_process_that_ignores_sigterm_after_the_grace() {
        let marker = marker();
        let straggler = Straggler::start(
            &mut Straggler::marked(Behaviour::IgnoresTerminate, &marker),
            1,
        );
        let pid = straggler.pid();

        let start = Instant::now();
        let terminated = marker.terminate(GRACE).unwrap();
        let elapsed = start.elapsed();

        assert!(
            straggler.stops(),
            "a process that ignores SIGTERM outlived terminate"
        );
        assert!(
            elapsed >= GRACE,
            "terminate killed after {elapsed:?}, before the grace ran out"
        );
        assert_eq!(terminated, vec![pid]);
    }
}
