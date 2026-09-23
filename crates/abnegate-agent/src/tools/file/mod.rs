//! Tools that read, write, list and search files beneath the working directory.

mod list;
mod patch;
mod read;
mod search;
#[cfg(test)]
mod tests;
mod walk;
mod write;

pub use list::ListFilesTool;
pub use patch::ApplyPatchTool;
pub use read::ReadFileTool;
pub use search::SearchCodeTool;
pub use write::WriteFileTool;

use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};

use super::{ToolContext, ToolError};

/// Refuse a resolved path that leaves `context.working_directory`.
///
/// The comparison is against the *canonical* `cwd`: a caller's `cwd` may itself
/// contain a symlink (`/var` -> `/private/var` on macOS), and a resolved path
/// compared against an unresolved root refuses every legitimate path in it.
pub(crate) fn confine(resolved: &Path, context: &ToolContext) -> Result<(), ToolError> {
    if context.unrestricted {
        return Ok(());
    }
    let root = context
        .working_directory
        .canonicalize()
        .unwrap_or_else(|_| context.working_directory.clone());
    if resolved.starts_with(&root) {
        Ok(())
    } else {
        Err(ToolError::Execution(
            "Path escapes working directory".to_string(),
        ))
    }
}

/// `path` with `.`, `..` and symlinks resolved as far as the filesystem allows.
///
/// A path that does not exist cannot be canonicalized, so its deepest existing
/// ancestor is resolved and the remaining names re-attached. That is what makes
/// a symlinked ancestor leaving `cwd` visible to [`confine`] *before* the
/// directories under it are created.
pub(crate) fn resolve(path: &Path) -> PathBuf {
    let lexical = normalize(path);
    let mut names: Vec<&OsStr> = Vec::new();
    let mut cursor = lexical.as_path();

    loop {
        if let Ok(canonical) = cursor.canonicalize() {
            let mut resolved = canonical;
            resolved.extend(names.iter().rev());
            return resolved;
        }
        match (cursor.parent(), cursor.file_name()) {
            (Some(parent), Some(name)) => {
                names.push(name);
                cursor = parent;
            }
            _ => return lexical,
        }
    }
}

fn normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                normalized.pop();
            }
            Component::CurDir => {}
            other => normalized.push(other),
        }
    }
    normalized
}
