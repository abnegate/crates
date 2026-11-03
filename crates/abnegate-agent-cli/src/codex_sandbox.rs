//! The sandbox Codex runs its own tools in.

use std::fmt;

/// The sandbox Codex runs the commands its model chooses in, set with
/// [`CliSettings::with_sandbox`](crate::CliSettings::with_sandbox).
///
/// `codex exec` never asks for approval, so its sandbox is all that confines
/// its own tools, and [`CodexSandbox::DangerFullAccess`] confines nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum CodexSandbox {
    /// Commands may read anything and change nothing.
    ReadOnly,
    /// Commands may change the working directory, the temporary directories
    /// Codex adds to it (`$TMPDIR`, and `/tmp` where it exists) and any
    /// writable roots Codex's own configuration names, and nothing else.
    WorkspaceWrite,
    /// Commands run unconfined.
    DangerFullAccess,
}

impl CodexSandbox {
    /// Every sandbox, from the most confining to the least.
    pub const ALL: &'static [Self] =
        &[Self::ReadOnly, Self::WorkspaceWrite, Self::DangerFullAccess];

    /// The sandbox's name as Codex's `--sandbox` and its `sandbox_mode`
    /// setting take it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ReadOnly => "read-only",
            Self::WorkspaceWrite => "workspace-write",
            Self::DangerFullAccess => "danger-full-access",
        }
    }

    /// The sandbox Codex calls `name`, spelled exactly as
    /// [`CodexSandbox::as_str`] spells it.
    pub fn named(name: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|sandbox| sandbox.as_str() == name)
    }
}

impl fmt::Display for CodexSandbox {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::CodexSandbox;

    #[test]
    fn a_sandbox_is_named_as_codex_spells_it() {
        assert_eq!(CodexSandbox::ReadOnly.as_str(), "read-only");
        assert_eq!(CodexSandbox::WorkspaceWrite.as_str(), "workspace-write");
        assert_eq!(
            CodexSandbox::DangerFullAccess.to_string(),
            "danger-full-access"
        );
        for sandbox in CodexSandbox::ALL {
            assert_eq!(CodexSandbox::named(sandbox.as_str()), Some(*sandbox));
        }
        for unknown in ["", "Workspace-Write", " workspace-write", "full"] {
            assert_eq!(CodexSandbox::named(unknown), None, "{unknown:?}");
        }
    }
}
