use std::process::ExitStatus;

/// A command that ran to its end, and what it wrote.
#[derive(Debug)]
pub(crate) struct Finished {
    pub(crate) status: ExitStatus,
    pub(crate) stdout: String,
    pub(crate) stderr: String,
}
