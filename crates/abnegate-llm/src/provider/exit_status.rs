use std::fmt;

/// How a child process ended.
///
/// A signalled process has no exit code, and reporting one as `-1` loses the
/// difference between a crash and a command that genuinely returned `-1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ExitStatus {
    Code(i32),
    Signalled,
}

impl fmt::Display for ExitStatus {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Code(code) => write!(formatter, "{code}"),
            Self::Signalled => formatter.write_str("signal"),
        }
    }
}

impl From<std::process::ExitStatus> for ExitStatus {
    fn from(status: std::process::ExitStatus) -> Self {
        status.code().map_or(Self::Signalled, Self::Code)
    }
}

#[cfg(test)]
mod tests {
    use super::ExitStatus;

    #[test]
    fn exit_status_keeps_a_signal_distinct_from_a_code() {
        assert_eq!(ExitStatus::Code(2).to_string(), "2");
        assert_eq!(ExitStatus::Signalled.to_string(), "signal");
    }
}
