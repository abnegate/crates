use std::path::PathBuf;

use serde::Deserialize;
use serde::Serialize;

use super::process_tree_request::ProcessTreeRequest;

/// Filesystem a confined job is allowed to see.
///
/// Everything outside these roots is denied, as is the network. Roots must be
/// absolute paths that exist when the job starts.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[non_exhaustive]
pub struct ConfinementRequest {
    /// Directories the job may read
    #[serde(default)]
    pub read_roots: Vec<PathBuf>,
    /// Directories the job may read and write
    #[serde(default)]
    pub write_roots: Vec<PathBuf>,
    /// Absent asks for the single-command confinement: the job runs one
    /// executable, which may neither fork nor exec anything else on a runner
    /// that advertises `confinement_single_process`. Present asks for a
    /// bounded process tree, which a build tool needs and a verification
    /// recipe does not, and which only a runner advertising
    /// `confinement_process_tree` accepts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process_tree: Option<ProcessTreeRequest>,
}

impl ConfinementRequest {
    /// Read `read_roots`, read and write `write_roots`, and run as a single
    /// command.
    pub fn new(read_roots: Vec<PathBuf>, write_roots: Vec<PathBuf>) -> Self {
        Self {
            read_roots,
            write_roots,
            process_tree: None,
        }
    }

    /// Run as a process tree bounded by `process_tree` instead of as a single
    /// command.
    pub fn with_process_tree(mut self, process_tree: ProcessTreeRequest) -> Self {
        self.process_tree = Some(process_tree);
        self
    }
}
