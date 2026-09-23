use std::collections::BTreeMap;
use std::path::PathBuf;

use super::error::ConfinementError;
use super::mode::ConfinementMode;
use super::path::canonical_roots;

pub(super) struct Resolved {
    pub(super) command: PathBuf,
    pub(super) arguments: Vec<String>,
    pub(super) working_dir: PathBuf,
    pub(super) read_roots: Vec<PathBuf>,
    pub(super) write_roots: Vec<PathBuf>,
    pub(super) execute_roots: Vec<PathBuf>,
    pub(super) mode: ConfinementMode,
    pub(super) environment: BTreeMap<String, String>,
}

/// Canonicalise and bound the executable directories a tree may launch from.
///
/// Single-command mode has no execute roots by construction. A tree needs at
/// least one, and none of them may be the filesystem root: an execute root of
/// `/` is "allow every exec on the host", which is the thing this mode exists
/// to avoid.
pub(super) fn resolve_execute_roots(
    mode: ConfinementMode,
    roots: &[PathBuf],
) -> Result<Vec<PathBuf>, ConfinementError> {
    if mode == ConfinementMode::SingleCommand {
        return Ok(Vec::new());
    }
    if roots.is_empty() {
        return Err(ConfinementError::ProcessTreeWithoutExecuteRoots);
    }

    let execute_roots = canonical_roots(roots)?;
    for root in &execute_roots {
        if root.parent().is_none() {
            return Err(ConfinementError::UnboundedExecuteRoot(
                root.display().to_string(),
            ));
        }
    }
    Ok(execute_roots)
}
