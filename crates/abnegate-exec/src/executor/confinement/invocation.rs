use std::collections::BTreeMap;
use std::path::PathBuf;

/// A backend executable and the argument vector that runs a command inside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
    pub program: PathBuf,
    pub arguments: Vec<String>,
    /// Environment for the backend process itself. Bubblewrap clears its own
    /// environment and carries the command's through `--setenv`, so this is
    /// empty there and complete under seatbelt.
    pub environment: BTreeMap<String, String>,
}
