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
mod index;
mod layout;
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
use crate::conflict::layout::Layout;
pub use crate::conflict::marker::BASE_MARKER;
pub use crate::conflict::marker::OURS_MARKER;
pub use crate::conflict::marker::SPLIT_MARKER;
pub use crate::conflict::marker::THEIRS_MARKER;
pub use crate::conflict::marker::has_markers;
pub use crate::conflict::request::ConflictRequest;
pub use crate::conflict::service::ConflictService;
use crate::repository_url::RepositoryUrl;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;
use tempfile::TempDir;

/// The ref a fetched head lands on inside the throwaway repository.
const HEAD_REF: &str = "refs/conflict/head";

/// The ref a fetched base lands on inside the throwaway repository.
const BASE_REF: &str = "refs/conflict/base";

/// The ref git records the commit being merged in.
const MERGE_HEAD: &str = "MERGE_HEAD";

/// A reproduced conflict, and the throwaway directory holding it.
///
/// The repository lives beside the checkout rather than inside it, so nothing
/// a repair writes into the checkout -- a `.git` of its own, a hook, a
/// configuration -- is read by the git commands that check and commit the
/// repair. Dropping this deletes the checkout, the repository and the
/// isolation directory a repair was pointed at, so a repair cannot leave a
/// half-merged tree or a stray home directory behind on disk.
#[derive(Debug)]
pub struct Conflict {
    #[allow(
        dead_code,
        reason = "held so the throwaway directory outlives the repair"
    )]
    root: TempDir,
    layout: Layout,
    remote: RepositoryUrl,
    head_branch: BranchName,
    head: CommitSha,
    base: CommitSha,
    files: Vec<ConflictedPath>,
    index: String,
}

impl Conflict {
    /// The checkout a repair edits.
    pub fn path(&self) -> &Path {
        &self.layout.checkout
    }

    /// The directory a repair may use as its home, cache and temporary space.
    pub fn isolation(&self) -> &Path {
        &self.layout.isolation
    }

    /// The repository the head was fetched from and the repair is published to.
    pub fn remote(&self) -> &RepositoryUrl {
        &self.remote
    }

    /// The branch the repair is published to.
    pub fn head_branch(&self) -> &BranchName {
        &self.head_branch
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

    /// Confirm the repository still holds the exact merge it was prepared with:
    /// HEAD on the head that was fetched, and the merge of the base still in
    /// progress.
    ///
    /// Run after a repair has edited the tree: a repair applied to a checkout
    /// somebody moved underneath it is not a repair of this conflict.
    pub async fn verify(&self, service: &ConflictService) -> ConflictResult<()> {
        service.verify_config(&self.layout).await?;
        let checked_out = service.rev_parse(&self.layout, "HEAD").await?;
        let head = service.rev_parse(&self.layout, HEAD_REF).await?;
        let base = service.rev_parse(&self.layout, BASE_REF).await?;
        let merging = service.rev_parse(&self.layout, MERGE_HEAD).await;

        let unmoved = checked_out == self.head
            && head == self.head
            && base == self.base
            && merging.is_ok_and(|merging| merging == self.base);
        match unmoved {
            true => Ok(()),
            false => Err(ConflictError::CheckoutMoved),
        }
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
