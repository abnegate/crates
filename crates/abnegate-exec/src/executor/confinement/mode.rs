use crate::protocol::ConfinementRequest;

/// How much of a process tree a confined job may create.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfinementMode {
    /// One executable runs and nothing else. Forking and further execs are
    /// denied outright.
    SingleCommand,
    /// The command may fork and exec, bounded by an explicit set of executable
    /// directories.
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
