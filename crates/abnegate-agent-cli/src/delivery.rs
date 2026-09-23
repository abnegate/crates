//! How a prompt reaches a coding agent.

/// How a prompt reaches the child process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Delivery {
    /// Written to the child's stdin, which is then closed.
    Stdin,
    /// Appended to the argument list.
    Argument,
}
