use std::io;
use std::process::ExitStatus;

/// Why a supervised child stopped being waited on.
pub(super) enum Ending {
    Exited(io::Result<ExitStatus>),
    TimedOut,
    Cancelled,
}
