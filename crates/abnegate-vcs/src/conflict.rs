//! Reproducing a pull request's merge conflict in a throwaway checkout.
//!
//! A repair is only worth running against the exact tree that conflicts. Rather
//! than reuse the workspace a run left behind — which has a branch checked out,
//! may have moved on, and is shared with whatever else is running — this builds a
//! fresh repository in a temporary directory, fetches only the two commits under
//! discussion, and reproduces the merge there. The directory is deleted when the
//! [`Conflict`] is dropped.
//!
//! Nothing here ever touches an existing repository, and nothing here ever uses
//! the stash: `refs/stash` is shared between worktrees, so a background repair
//! that stashed would corrupt whatever else was running beside it.

mod conflicted_path;
mod error;
mod marker;
mod request;
mod service;

pub use crate::branch_name::BranchName;
pub use crate::commit_sha::CommitSha;
pub use crate::conflict::conflicted_path::ConflictedPath;
pub use crate::conflict::conflicted_path::resolve;
pub use crate::conflict::conflicted_path::validate;
pub use crate::conflict::error::ConflictError;
pub use crate::conflict::error::ConflictResult;
pub use crate::conflict::marker::BASE_MARKER;
pub use crate::conflict::marker::OURS_MARKER;
pub use crate::conflict::marker::SPLIT_MARKER;
pub use crate::conflict::marker::THEIRS_MARKER;
pub use crate::conflict::marker::has_markers;
pub use crate::conflict::request::ConflictRequest;
pub use crate::conflict::service::ConflictService;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;
use tempfile::TempDir;

/// The ref a fetched head lands on inside the throwaway checkout.
pub(crate) const HEAD_REF: &str = "refs/conflict/head";

/// The ref a fetched base lands on inside the throwaway checkout.
pub(crate) const BASE_REF: &str = "refs/conflict/base";

/// A reproduced conflict, and the throwaway directory holding it.
///
/// Dropping this deletes both the checkout and the isolation directory a repair
/// was pointed at, so a repair cannot leave a half-merged tree or a stray home
/// directory behind on disk.
#[derive(Debug)]
pub struct Conflict {
    #[allow(
        dead_code,
        reason = "held so the throwaway directory outlives the repair"
    )]
    root: TempDir,
    checkout_path: PathBuf,
    isolation_path: PathBuf,
    head: CommitSha,
    base: CommitSha,
    files: Vec<ConflictedPath>,
}

impl Conflict {
    pub fn path(&self) -> &Path {
        &self.checkout_path
    }

    /// The directory a repair may use as its home, cache and temporary space.
    pub fn isolation(&self) -> &Path {
        &self.isolation_path
    }

    pub fn head(&self) -> &CommitSha {
        &self.head
    }

    pub fn base(&self) -> &CommitSha {
        &self.base
    }

    pub fn files(&self) -> &[ConflictedPath] {
        &self.files
    }

    /// The absolute path of one conflicted file, refusing anything outside the set.
    ///
    /// The candidate may be repository-relative or absolute inside the checkout;
    /// either way it has to name a file git reported as unmerged.
    pub fn confine(&self, candidate: &str) -> ConflictResult<PathBuf> {
        let relative = self.relative(candidate)?;
        if !self.files.contains(&relative) {
            return Err(ConflictError::UnsafePath(candidate.to_string()));
        }
        resolve(self.path(), &relative)
    }

    fn relative(&self, candidate: &str) -> ConflictResult<ConflictedPath> {
        let trimmed = candidate.trim();
        if trimmed.is_empty() {
            return Err(ConflictError::UnsafePath(candidate.to_string()));
        }

        let as_path = Path::new(trimmed);
        let within = match as_path.is_absolute() {
            true => {
                let root = self
                    .path()
                    .canonicalize()
                    .map_err(|_| ConflictError::UnsafePath(candidate.to_string()))?;
                let normalized = normalize(as_path)
                    .ok_or_else(|| ConflictError::UnsafePath(candidate.to_string()))?;
                normalized
                    .strip_prefix(&root)
                    .map_err(|_| ConflictError::UnsafePath(candidate.to_string()))?
                    .to_path_buf()
            }
            false => PathBuf::from(trimmed),
        };

        let text = within
            .to_str()
            .ok_or_else(|| ConflictError::UnsafePath(candidate.to_string()))?
            .replace(std::path::MAIN_SEPARATOR, "/");
        ConflictedPath::parse(&text)
    }

    /// Confirm the checkout still holds the exact commits it was prepared with.
    ///
    /// Run after an agent has edited the tree: a repair that was applied to a
    /// checkout somebody moved underneath it is not a repair of this conflict.
    pub async fn verify(&self, service: &ConflictService) -> ConflictResult<()> {
        let checked_out = service.rev_parse(self.path(), "HEAD").await?;
        let head = service.rev_parse(self.path(), HEAD_REF).await?;
        let base = service.rev_parse(self.path(), BASE_REF).await?;

        if checked_out != self.head || head != self.head || base != self.base {
            return Err(ConflictError::CheckoutMoved);
        }
        Ok(())
    }
}

/// Lexically resolve `.` and `..` without touching the filesystem.
fn normalize(path: &Path) -> Option<PathBuf> {
    let mut resolved = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                if !resolved.pop() {
                    return None;
                }
            }
            Component::CurDir => {}
            other => resolved.push(other),
        }
    }
    Some(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_is_resolved_lexically_or_refused_for_climbing_out() {
        assert_eq!(
            normalize(Path::new("/a/./b/../c")),
            Some(PathBuf::from("/a/c"))
        );
        assert_eq!(normalize(Path::new("a/../..")), None);
    }
}
