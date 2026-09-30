use std::fmt;

/// A kind of entry in a run's [`Journal`](crate::log::Journal).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Record {
    /// The run's settings, before the agent is started.
    Initialized,
    /// The agent's process could not be started.
    SpawnFailed,
    /// The agent's process started.
    Spawned,
    /// One line the agent printed to stdout.
    StdoutLine,
    /// The agent closed its stdout.
    StdoutClosed,
    /// Reading the agent's stdout failed.
    StdoutFailed,
    /// An event too long to read that the run could do without was skipped.
    StdoutDropped,
    /// One line the agent printed to stderr.
    StderrLine,
    /// The agent closed its stderr.
    StderrClosed,
    /// The run was abandoned while the agent was still running: it reported
    /// a failure, or its output broke the stream's limits.
    Abandoned,
    /// The agent's process exited, with its status.
    Exited,
    /// Waiting for the agent's process to exit failed.
    WaitFailed,
    /// The run outlived [`CliSettings::timeout`](crate::CliSettings::timeout)
    /// and was stopped.
    TimedOut,
    /// The agent was stopped because it did not exit once its output had
    /// settled the run.
    Stopped,
    /// The run's outcome, once the agent is gone and its output parsed.
    Completed,
    /// The journal reached its limit, and the lines printed after this are
    /// not recorded.
    Truncated,
}

impl Record {
    /// The entry's `event` name in the journal, such as `stdout_line`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Initialized => "execution_initialized",
            Self::SpawnFailed => "spawn_failed",
            Self::Spawned => "subprocess_spawned",
            Self::StdoutLine => "stdout_line",
            Self::StdoutClosed => "stdout_stream_closed",
            Self::StdoutFailed => "stdout_read_error",
            Self::StdoutDropped => "stdout_line_dropped",
            Self::StderrLine => "stderr_line",
            Self::StderrClosed => "stderr_stream_closed",
            Self::Abandoned => "subprocess_early_failure",
            Self::Exited => "subprocess_exited",
            Self::WaitFailed => "wait_failed",
            Self::TimedOut => "subprocess_timed_out",
            Self::Stopped => "subprocess_terminated_after_early_failure",
            Self::Completed => "process_completed",
            Self::Truncated => "journal_truncated",
        }
    }
}

impl fmt::Display for Record {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::Record;

    #[test]
    fn every_record_has_a_distinct_snake_case_name() {
        let records = [
            Record::Initialized,
            Record::SpawnFailed,
            Record::Spawned,
            Record::StdoutLine,
            Record::StdoutClosed,
            Record::StdoutFailed,
            Record::StdoutDropped,
            Record::StderrLine,
            Record::StderrClosed,
            Record::Abandoned,
            Record::Exited,
            Record::WaitFailed,
            Record::TimedOut,
            Record::Stopped,
            Record::Completed,
            Record::Truncated,
        ];
        let mut names: Vec<&str> = records.iter().map(|record| record.as_str()).collect();
        for name in &names {
            assert!(
                name.chars()
                    .all(|character| character.is_ascii_lowercase() || character == '_'),
                "{name}"
            );
        }
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), records.len());
        assert_eq!(Record::StdoutLine.to_string(), "stdout_line");
    }
}
