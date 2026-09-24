use std::path::PathBuf;

use serde::Deserialize;
use serde::Serialize;

/// The bound on a confined process tree.
///
/// A tree may only start executables that live under one of `execute_roots`,
/// plus the entry command itself. Read and write roots never confer the right
/// to execute, so a tree cannot write a binary into its workspace and then run
/// it unless the caller named that directory here. A runner whose sandbox
/// cannot hold that bound does not advertise `confinement_process_tree` and
/// refuses the job with `confinement_unavailable`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[non_exhaustive]
pub struct ProcessTreeRequest {
    /// Directories whose executables the tree may run
    #[serde(default)]
    pub execute_roots: Vec<PathBuf>,
}

impl ProcessTreeRequest {
    /// A tree that may run the executables under `execute_roots`.
    pub fn new(execute_roots: Vec<PathBuf>) -> Self {
        Self { execute_roots }
    }
}
