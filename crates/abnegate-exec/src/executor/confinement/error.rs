use thiserror::Error;

/// Errors raised while preparing or proving confinement.
#[derive(Debug, Clone, Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum ConfinementError {
    /// The host has no confinement backend at all
    #[error("Confinement is not supported on this platform")]
    UnsupportedPlatform,

    /// The backend executable is missing, not a file, or not executable
    #[error("Confinement backend is unusable: {path}: {reason}")]
    BackendUnusable { path: String, reason: String },

    /// A root or command path is relative
    #[error("Confined path must be absolute: {0}")]
    RelativePath(String),

    /// A path cannot be written into a sandbox profile
    #[error("Confined path is not valid UTF-8: {0}")]
    NonUnicodePath(String),

    /// A path would break out of the profile line it is written into
    #[error("Confined path contains a control character: {0}")]
    ControlCharacterInPath(String),

    /// A path does not exist or cannot be canonicalised
    #[error("Confined path is unusable: {path}: {reason}")]
    UnusablePath { path: String, reason: String },

    /// The command is not an executable file on the search path
    #[error("Confined command not found: {0}")]
    CommandNotFound(String),

    /// A tree was requested with nothing it may execute
    #[error("A confined process tree needs at least one execute root")]
    ProcessTreeWithoutExecuteRoots,

    /// An execute root of `/`, which bounds nothing
    #[error("Execute root would admit every executable on the host: {0}")]
    UnboundedExecuteRoot(String),

    /// The probe could not show the sandbox holds what the mode claims
    #[error("Confinement could not be proven: {0}")]
    Unproven(String),
}
