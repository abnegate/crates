/// Runner capabilities advertised during handshake
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capability {
    /// Can cancel running jobs
    Cancel,
    /// Can send stdin to running jobs
    Stdin,
    /// Emits structured log messages
    Logs,
    /// Uses process groups for clean kill
    ProcessGroup,
    /// Can run jobs under OS-level confinement
    Confinement,
    /// Can confine a whole process tree, not just a single executable
    ConfinementProcessTree,
}

impl Capability {
    pub(crate) const ALL: [Capability; 6] = [
        Capability::Cancel,
        Capability::Stdin,
        Capability::Logs,
        Capability::ProcessGroup,
        Capability::Confinement,
        Capability::ConfinementProcessTree,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            Capability::Cancel => "cancel",
            Capability::Stdin => "stdin",
            Capability::Logs => "logs",
            Capability::ProcessGroup => "process_group",
            Capability::Confinement => "confinement",
            Capability::ConfinementProcessTree => "confinement_process_tree",
        }
    }

    /// Every capability the protocol defines.
    pub fn all() -> Vec<String> {
        Self::ALL
            .iter()
            .map(|capability| capability.as_str().to_string())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_capability_as_str() {
        assert_eq!(Capability::Cancel.as_str(), "cancel");
        assert_eq!(Capability::Stdin.as_str(), "stdin");
        assert_eq!(Capability::Logs.as_str(), "logs");
        assert_eq!(Capability::ProcessGroup.as_str(), "process_group");
        assert_eq!(Capability::Confinement.as_str(), "confinement");
        assert_eq!(
            Capability::ConfinementProcessTree.as_str(),
            "confinement_process_tree"
        );
    }

    #[test]
    fn test_capability_all() {
        let all = Capability::all();
        assert_eq!(all.len(), 6);
        assert!(all.contains(&"cancel".to_string()));
        assert!(all.contains(&"stdin".to_string()));
        assert!(all.contains(&"logs".to_string()));
        assert!(all.contains(&"process_group".to_string()));
        assert!(all.contains(&"confinement".to_string()));
        assert!(all.contains(&"confinement_process_tree".to_string()));
    }
}
