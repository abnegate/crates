use crate::protocol::ConfinementRequest;

/// How much of a process tree a confined job may create.
///
/// Both modes confine the filesystem to the granted roots and deny the
/// network on every backend. What else a mode holds depends on the backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ConfinementMode {
    /// One executable runs. Where the backend
    /// [enforces a single process](super::Backend::enforces_single_process),
    /// forking and further execs are denied outright; elsewhere the command
    /// may fork and exec within the same confinement.
    SingleCommand,
    /// The command may fork and exec, bounded by an explicit set of executable
    /// directories. Only a backend that
    /// [enforces execute roots](super::Backend::enforces_execute_roots) can
    /// prove that bound, and any other refuses the job.
    ProcessTree,
}

impl ConfinementRequest {
    /// How much of a process tree this request asks for.
    pub fn mode(&self) -> ConfinementMode {
        match self.process_tree {
            Some(_) => ConfinementMode::ProcessTree,
            None => ConfinementMode::SingleCommand,
        }
    }
}
