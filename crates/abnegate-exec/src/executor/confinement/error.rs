use thiserror::Error;

/// Errors raised while preparing or proving confinement.
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum ConfinementError {
    #[error("Confinement is not supported on this platform")]
    UnsupportedPlatform,

    #[error("Confinement backend is unusable: {path}: {reason}")]
    BackendUnusable { path: String, reason: String },

    #[error("Confined path must be absolute: {0}")]
    RelativePath(String),

    #[error("Confined path is not valid UTF-8: {0}")]
    NonUnicodePath(String),

    #[error("Confined path contains a control character: {0}")]
    ControlCharacterInPath(String),

    #[error("Confined path is unusable: {path}: {reason}")]
    UnusablePath { path: String, reason: String },

    #[error("Confined command not found: {0}")]
    CommandNotFound(String),

    #[error("A confined process tree needs at least one execute root")]
    ProcessTreeWithoutExecuteRoots,

    #[error("Execute root would admit every executable on the host: {0}")]
    UnboundedExecuteRoot(String),

    #[error("Confinement could not be proven: {0}")]
    Unproven(String),
}
