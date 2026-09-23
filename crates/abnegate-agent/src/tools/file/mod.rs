//! Tools that read, write, list and search files beneath the working directory.

mod list;
mod patch;
mod read;
mod search;
#[cfg(test)]
mod tests;
mod walk;
mod write;

use std::ffi::OsStr;
use std::io;
use std::io::Read;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;

pub use list::ListFilesTool;
pub use patch::ApplyPatchTool;
pub use read::ReadFileTool;
pub use search::SearchCodeTool;
pub use write::WriteFileTool;

use super::ToolContext;
use super::ToolError;
use super::beneath;
use super::beneath::Access;

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

/// The text of the file at `path`, refused past the context's
/// `max_file_size`.
///
/// The limit is held on what is read as well as on the size the file reports,
/// so a file growing while it is read is refused rather than cut short. This
/// blocks, so it runs off the async workers: through [`blocking`], or inside a
/// blocking task of the caller's own.
pub(super) fn read_text(context: &ToolContext, path: &Path) -> Result<String, ToolError> {
    let limit = context.max_file_size as u64;
    let file = beneath::open(context, path, Access::Read)?;
    let size = file.metadata().map_err(unreadable)?.len();
    if size > limit {
        return Err(too_large(size, context));
    }
    let mut bytes = Vec::with_capacity(size as usize);
    file.take(limit.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(unreadable)?;
    if bytes.len() as u64 > limit {
        return Err(too_large(bytes.len() as u64, context));
    }
    String::from_utf8(bytes).map_err(|_| {
        unreadable(io::Error::new(
            io::ErrorKind::InvalidData,
            "stream did not contain valid UTF-8",
        ))
    })
}

fn too_large(size: u64, context: &ToolContext) -> ToolError {
    ToolError::Execution(format!(
        "File too large ({size} bytes, max {})",
        context.max_file_size
    ))
}

fn unreadable(error: io::Error) -> ToolError {
    ToolError::Execution(format!("Cannot read file: {error}"))
}

/// Run file work on the blocking pool, where a slow disk holds a thread of
/// its own rather than an async worker the rest of the run is waiting on.
pub(super) async fn blocking<Value: Send + 'static>(
    work: impl FnOnce() -> Result<Value, ToolError> + Send + 'static,
) -> Result<Value, ToolError> {
    tokio::task::spawn_blocking(work).await.map_err(|error| {
        ToolError::Execution(format!("The file operation did not finish: {error}"))
    })?
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
