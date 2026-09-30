//! Tools that read, write, list and search files beneath the working directory.

mod denied;
mod identity;
mod list;
mod patch;
mod read;
mod search;
#[cfg(test)]
mod tests;
mod walk;
mod withheld;
mod write;

use std::collections::VecDeque;
use std::fs;
use std::io;
use std::io::Read;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;

pub use list::ListFilesTool;
pub use patch::ApplyPatchTool;
pub use read::ReadFileTool;
pub use search::SearchCodeTool;
use withheld::Withheld;
use withheld::per_process;
pub use write::WriteFileTool;

use super::ToolContext;
use super::ToolError;
use super::beneath;
use super::beneath::Access;

/// What a file tool answers for a path in [`ToolContext::denied`], or in a
/// directory that reaches a file by identity, however unrestricted its
/// context.
pub const OFF_LIMITS: &str = "Path is off limits to file tools";

const TOO_MANY_LINKS: &str = "Path runs through too many symbolic links";

/// Refuse a resolved path a file tool may not reach: one that is withheld from
/// every file tool, and unless the context is unrestricted, one that leaves
/// `context.working_directory`.
///
/// The comparison is against the *canonical* `cwd`: a caller's `cwd` may itself
/// contain a symlink (`/var` -> `/private/var` on macOS), and a resolved path
/// compared against an unresolved root refuses every legitimate path in it.
pub(crate) fn confine(resolved: &Path, context: &ToolContext) -> Result<(), ToolError> {
    if Withheld::of(context).holds(resolved) {
        return Err(ToolError::Execution(OFF_LIMITS.to_string()));
    }
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
/// `maximum_file_size`.
///
/// The limit is held on what is read as well as on the size the file reports,
/// so a file growing while it is read is refused rather than cut short. This
/// blocks, so it runs off the async workers: through [`blocking`], or inside a
/// blocking task of the caller's own.
pub(super) fn read_text(context: &ToolContext, path: &Path) -> Result<String, ToolError> {
    let limit = context.maximum_file_size as u64;
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
        context.maximum_file_size
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

/// `path` with `.`, `..` and symlinks resolved the way the kernel resolves
/// them, as far as the filesystem allows.
///
/// Each name is looked up where the names before it really led, so a `..`
/// after a symlink leaves the link's target rather than the link. A link is
/// followed whether or not its target exists, since a create through it lands
/// there, and names past the deepest one that exists are taken as written.
/// That is what makes a symlinked ancestor leaving the working directory
/// visible to [`confine`] *before* the directories under it are created.
/// Nothing past a process's `/proc` entry is resolved, since [`confine`]
/// refuses it whole. A path that needs more links followed than the kernel
/// would follow is refused, as the kernel refuses it: kept as written, its
/// last link would pass for a directory inside whatever holds it.
pub(crate) fn resolve(path: &Path) -> Result<PathBuf, ToolError> {
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let mut pending = steps(&absolute);
    let mut resolved = PathBuf::new();
    let mut links = beneath::LINKS;

    while let Some(step) = pending.pop_front() {
        if per_process(&resolved) {
            resolved.push(step);
            resolved.extend(pending);
            break;
        }
        match step.components().next() {
            Some(Component::ParentDir) => {
                resolved.pop();
            }
            Some(Component::Normal(name)) => {
                let candidate = resolved.join(name);
                match fs::read_link(&candidate) {
                    Ok(target) => {
                        links = links
                            .checked_sub(1)
                            .ok_or_else(|| ToolError::Execution(TOO_MANY_LINKS.to_string()))?;
                        for step in steps(&target).into_iter().rev() {
                            pending.push_front(step);
                        }
                    }
                    Err(_) => resolved = candidate,
                }
            }
            Some(Component::CurDir) | None => {}
            Some(root) => resolved.push(root),
        }
    }
    Ok(resolved)
}

/// Each component of `path`, owned, so a link's target can be spliced in.
fn steps(path: &Path) -> VecDeque<PathBuf> {
    path.components()
        .map(|component| PathBuf::from(component.as_os_str()))
        .collect()
}
