use super::*;

/// The suffix a directory holding worktrees has to carry before
/// [`GitService::remove_worktree`] will delete anything inside it by hand.
const WORKTREE_AREA_SUFFIX: &str = "-worktrees";

impl GitService {
    /// Add a worktree of a managed clone at `worktree_path`, in detached HEAD
    /// state at `checkout_ref`.
    ///
    /// A directory already standing there is removed first, so a worktree a
    /// crashed run left behind does not refuse the next one.
    pub async fn create_worktree(
        &self,
        path: &Path,
        worktree_path: &Path,
        checkout_ref: &BranchName,
    ) -> GitResult<()> {
        let worktree_path = Self::make_absolute(worktree_path)?;
        let worktree_path = worktree_path.as_path();

        if worktree_path.exists() {
            tracing::warn!(worktree = ?worktree_path, "Stale worktree found, removing");
            self.remove_worktree(path, worktree_path).await?;
        }

        if let Some(parent) = worktree_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        tracing::info!(
            repository = ?path,
            worktree = ?worktree_path,
            %checkout_ref,
            "Creating worktree"
        );

        let output = Self::output(
            Self::managed_command(Some(path))
                .args(["worktree", "add", "--detach", "--"])
                .arg(worktree_path)
                .arg(checkout_ref.as_str()),
        )
        .await?;

        if !output.status.success() {
            return Err(GitError::CommandFailed(format!(
                "git worktree add failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }

        tracing::info!(worktree = ?worktree_path, "Worktree created");
        Ok(())
    }

    /// Add a worktree of a managed clone checked out on a local branch.
    ///
    /// Unlike [`Self::create_worktree`], which detaches, this creates or resets
    /// `branch` at `start_point`, so a later push from the worktree targets the
    /// branch the caller named.
    pub async fn create_worktree_on_branch(
        &self,
        path: &Path,
        worktree_path: &Path,
        branch: &BranchName,
        start_point: &BranchName,
    ) -> GitResult<()> {
        let worktree_path = Self::make_absolute(worktree_path)?;
        let worktree_path = worktree_path.as_path();

        if worktree_path.exists() {
            tracing::warn!(worktree = ?worktree_path, "Stale worktree found, removing");
            self.remove_worktree(path, worktree_path).await?;
        }

        if let Some(parent) = worktree_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        tracing::info!(
            repository = ?path,
            worktree = ?worktree_path,
            %branch,
            %start_point,
            "Creating worktree on branch"
        );

        let output = Self::output(
            Self::managed_command(Some(path))
                .args(["worktree", "add", "-B", branch.as_str(), "--"])
                .arg(worktree_path)
                .arg(start_point.as_str()),
        )
        .await?;

        if !output.status.success() {
            return Err(GitError::CommandFailed(format!(
                "git worktree add -B {branch} failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }

        tracing::info!(worktree = ?worktree_path, %branch, "Worktree created on branch");
        Ok(())
    }

    /// Remove a worktree of a managed clone and prune the record of it.
    ///
    /// A directory git refuses to let go of is deleted by hand, but only inside
    /// a `*-worktrees` area: everything else is somewhere the caller did not
    /// declare disposable.
    pub async fn remove_worktree(&self, path: &Path, worktree_path: &Path) -> GitResult<()> {
        let worktree_path = Self::make_absolute(worktree_path)?;
        let worktree_path = worktree_path.as_path();

        tracing::debug!(repository = ?path, worktree = ?worktree_path, "Removing worktree");

        let output = Self::output(
            Self::managed_command(Some(path))
                .args(["worktree", "remove", "--force", "--"])
                .arg(worktree_path),
        )
        .await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            tracing::warn!(error = %stderr, "git worktree remove failed, deleting the directory");
        }

        if worktree_path.exists() {
            let area = worktree_path
                .parent()
                .and_then(Path::file_name)
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            if !area.ends_with(WORKTREE_AREA_SUFFIX) {
                return Err(GitError::UnsafeWorktree(worktree_path.to_path_buf()));
            }
            tokio::fs::remove_dir_all(worktree_path).await?;
        }

        let _ = Self::output(Self::managed_command(Some(path)).args(["worktree", "prune"])).await;

        tracing::debug!(worktree = ?worktree_path, "Worktree removed");
        Ok(())
    }
}
