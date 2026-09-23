use std::io;

/// Why a supervised child stopped being waited on.
pub(super) enum Ending {
    /// The child exited; it is not yet reaped.
    Exited(io::Result<()>),
    TimedOut,
    Cancelled,
}
