use super::*;

impl GitService {
    /// A git invocation against a clone the caller owns outright, run with the
    /// caller's own environment and configuration, for the clone that creates
    /// it -- so an address only that setup can reach, a local path, an SSH
    /// remote, a credential helper, still works -- and for the local reads and
    /// configuration writes that run nothing a repository names.
    pub(super) fn managed_command(path: Option<&Path>) -> Command {
        let mut command = Command::new("git");
        command
            .env("GIT_TERMINAL_PROMPT", "0")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if let Some(path) = path {
            command.current_dir(path);
        }
        command
    }

    /// A hardened git invocation against a managed clone's own repository, for
    /// the local checkout, reset, worktree and `rev-parse` operations that
    /// read the clone's shared configuration file and hooks.
    ///
    /// Every worktree of a managed clone shares those, and a run works in a
    /// worktree, so a run can leave a hook, a file-system monitor or a driver
    /// there that the next such operation would otherwise run as the host with
    /// the caller's environment. These operations are local, so they run with
    /// the host configuration ignored, those settings pinned off, and no
    /// transport at all; only [`Self::verify_config`], run first, guards a key
    /// no pin reaches. The fetches that reach the caller's configured address
    /// keep the caller's environment through [`Self::managed_remote`].
    pub(super) fn managed_local(path: &Path) -> Command {
        let mut command = Self::hardened();
        command
            .env("GIT_ALLOW_PROTOCOL", "")
            .current_dir(path)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }

    /// A git invocation that reaches a managed clone's configured remote with
    /// the caller's own environment, so a local path or an SSH remote that
    /// environment reaches still works, under every pin a hardened command
    /// carries.
    ///
    /// The clone's hooks and configuration are shared by every worktree of it,
    /// and a run works in one, so a hook a run left would otherwise run as the
    /// host on the next fetch; [`Self::verify_config`], run first, refuses a key
    /// no pin reaches. The pins override the caller's own configuration too: a
    /// credential helper, proxy or redirect policy the caller configured is not
    /// used, and a remote is reached with what the environment itself supplies.
    pub(super) fn managed_remote(path: &Path) -> Command {
        let mut command = Self::managed_command(Some(path));
        command.args(PINS);
        command
    }

    /// Every branch `origin` has, and none it no longer has.
    fn fetching_all(path: &Path) -> Command {
        let mut command = Self::managed_remote(path);
        command.args(["fetch", "--prune", "--", ORIGIN]);
        command
    }

    /// One branch of `origin`.
    fn fetching(path: &Path, branch: &BranchName) -> Command {
        let mut command = Self::managed_remote(path);
        command.args(["fetch", "--", ORIGIN, branch.as_str()]);
        command
    }

    /// `refs/remotes/origin/HEAD` pointed at the branch `origin` reports.
    fn setting_head(path: &Path) -> Command {
        let mut command = Self::managed_remote(path);
        command.args(["remote", "set-head", "--auto", "--", ORIGIN]);
        command
    }

    /// `branch` checked out at its remote-tracking ref, whatever it held.
    fn checking_out(path: &Path, branch: &BranchName) -> Command {
        let mut command = Self::managed_local(path);
        command.args([
            "checkout",
            "-f",
            "-B",
            branch.as_str(),
            &format!("{REMOTE_TRACKING}{branch}"),
            "--",
        ]);
        command
    }

    /// The index and working tree reset to `branch`'s remote-tracking ref.
    fn resetting(path: &Path, branch: &BranchName) -> Command {
        let mut command = Self::managed_local(path);
        command.args([
            "reset",
            "--hard",
            &format!("{REMOTE_TRACKING}{branch}"),
            "--",
        ]);
        command
    }

    /// Convert a possibly relative path to an absolute one using the process
    /// working directory.
    ///
    /// Managed commands set `current_dir` to the repository, so a relative path
    /// passed as an argument would otherwise resolve against the repository
    /// rather than against the directory the caller was standing in.
    pub(super) fn make_absolute(path: &Path) -> GitResult<PathBuf> {
        match path.is_absolute() {
            true => Ok(path.to_path_buf()),
            false => Ok(std::env::current_dir()?.join(path)),
        }
    }

    /// Ensure a managed clone exists at `path` and is up to date on
    /// `default_branch`, cloning it from `url` when it is not there yet.
    pub async fn ensure_repository(
        &self,
        path: &Path,
        url: &str,
        default_branch: &BranchName,
    ) -> GitResult<()> {
        match path.exists() {
            true => self.pull(path, default_branch).await,
            false => self.clone_managed(url, path).await,
        }
    }

    /// Ensure a managed clone's object store is current without checking out or
    /// resetting anything, and say what the remote's default branch is.
    ///
    /// `refs/remotes/origin/HEAD` is refreshed first so the answer reflects what
    /// the remote reports rather than what the clone was last told.
    pub async fn ensure_fetched(&self, path: &Path, url: &str) -> GitResult<BranchName> {
        match path.exists() {
            true => self.fetch_all(path).await?,
            false => self.clone_managed(url, path).await?,
        }

        self.update_remote_head(path).await;

        Ok(self.detect_default_branch(path).await)
    }

    /// Ensure a managed clone is current *and* its working tree is advanced to
    /// the remote's default branch.
    pub async fn ensure_synced(&self, path: &Path, url: &str) -> GitResult<BranchName> {
        if self.is_repository_root(path) {
            self.track_all_branches(path).await;
        }
        let default_branch = self.ensure_fetched(path, url).await?;
        self.checkout_reset(path, &default_branch).await?;
        Ok(default_branch)
    }

    /// Widen the fetch refspec to every branch the remote has.
    async fn track_all_branches(&self, path: &Path) {
        let output = Self::output(Self::managed_command(Some(path)).args([
            "remote",
            "set-branches",
            "--",
            ORIGIN,
            "*",
        ]))
        .await;
        match output {
            Ok(result) if result.status.success() => {}
            Ok(result) => {
                let stderr = String::from_utf8_lossy(&result.stderr);
                tracing::debug!(repository = ?path, error = %stderr, "Failed to widen fetch refspec");
            }
            Err(error) => {
                tracing::debug!(repository = ?path, %error, "Failed to run git remote set-branches");
            }
        }
    }

    /// Fetch every remote ref into a managed clone without touching its
    /// working tree. A clone whose configuration holds anything beyond what
    /// git writes for one is refused with [`GitError::UnsafeConfig`] first.
    pub async fn fetch_all(&self, path: &Path) -> GitResult<()> {
        tracing::debug!(repository = ?path, "Fetching all remote refs");

        Self::verify_config(path).await?;
        let output = Self::output(&mut Self::fetching_all(path)).await?;

        if !output.status.success() {
            return Err(GitError::CommandFailed(format!(
                "git fetch failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }

        tracing::debug!(repository = ?path, "Fetch completed");
        Ok(())
    }

    /// Fetch one branch from `origin` into a managed clone, refusing one
    /// configured as [`Self::fetch_all`] refuses it.
    pub async fn fetch_branch(&self, path: &Path, branch: &BranchName) -> GitResult<()> {
        tracing::debug!(repository = ?path, %branch, "Fetching branch");

        Self::verify_config(path).await?;
        let output = Self::output(&mut Self::fetching(path, branch)).await?;

        if !output.status.success() {
            return Err(GitError::CommandFailed(format!(
                "git fetch branch failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }

        Ok(())
    }

    /// Clone a managed repository from any address the caller can reach.
    async fn clone_managed(&self, url: &str, target: &Path) -> GitResult<()> {
        tracing::info!(url, target = ?target, "Cloning repository");

        if let Some(parent) = target.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let output = Self::output(
            Self::managed_command(None)
                .args(["clone", "--", url])
                .arg(target),
        )
        .await?;

        if !output.status.success() {
            return Err(GitError::CommandFailed(format!(
                "git clone failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }

        tracing::info!(target = ?target, "Repository cloned successfully");
        Ok(())
    }

    /// Fetch `branch` and advance a managed clone's working tree to it.
    async fn pull(&self, path: &Path, branch: &BranchName) -> GitResult<()> {
        tracing::debug!(repository = ?path, %branch, "Pulling latest changes");

        Self::verify_config(path).await?;
        let output = Self::output(&mut Self::fetching(path, branch)).await?;

        if !output.status.success() {
            return Err(GitError::CommandFailed(format!(
                "git fetch failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }

        self.checkout_reset(path, branch).await?;

        tracing::debug!(repository = ?path, "Repository updated successfully");
        Ok(())
    }

    /// Check out `branch` and hard-reset the working tree to `origin/<branch>`.
    ///
    /// Assumes the refs are already fetched, and discards anything the working
    /// tree holds: only a managed clone may be reset this way. Both steps are
    /// local, so they run hardened, after the clone's configuration is checked.
    async fn checkout_reset(&self, path: &Path, branch: &BranchName) -> GitResult<()> {
        Self::verify_config(path).await?;
        let output = Self::output(&mut Self::checking_out(path, branch)).await?;

        if !output.status.success() {
            return Err(GitError::CommandFailed(format!(
                "git checkout failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }

        let output = Self::output(&mut Self::resetting(path, branch)).await?;

        if !output.status.success() {
            return Err(GitError::CommandFailed(format!(
                "git reset failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }

        Ok(())
    }

    /// Whether `path` is the root of a repository or of a worktree, read from
    /// the file system rather than by running git.
    pub fn is_repository_root(&self, path: &Path) -> bool {
        let marker = path.join(".git");
        marker.is_dir() || marker.is_file()
    }

    /// The remote's default branch, read from `refs/remotes/origin/HEAD`, or
    /// `main` when the ref cannot be read.
    pub async fn detect_default_branch(&self, path: &Path) -> BranchName {
        let output =
            Self::output(Self::managed_command(Some(path)).args(["symbolic-ref", REMOTE_HEAD]))
                .await;

        match output {
            Ok(result) if result.status.success() => default_branch_of(&result.stdout),
            _ => fallback_default_branch(),
        }
    }

    /// [`Self::detect_default_branch`] for a caller that cannot await, such as
    /// one building a file-system index.
    pub fn detect_default_branch_blocking(&self, path: &Path) -> BranchName {
        let output = std::process::Command::new("git")
            .args(["symbolic-ref", REMOTE_HEAD])
            .current_dir(path)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output();

        match output {
            Ok(result) if result.status.success() => default_branch_of(&result.stdout),
            _ => fallback_default_branch(),
        }
    }

    /// Point `refs/remotes/origin/HEAD` at whatever the remote reports as its
    /// default branch. Best effort: it reaches the network, and a caller that
    /// cannot reach it is no worse off than before. A clone configured as
    /// [`Self::fetch_all`] refuses is left as it was.
    async fn update_remote_head(&self, path: &Path) {
        if let Err(error) = Self::verify_config(path).await {
            tracing::warn!(repository = ?path, %error, "Refusing to update origin/HEAD");
            return;
        }
        let output = Self::output(&mut Self::setting_head(path)).await;

        match output {
            Ok(result) if result.status.success() => {
                tracing::debug!(repository = ?path, "Updated origin/HEAD");
            }
            Ok(result) => {
                let stderr = String::from_utf8_lossy(&result.stderr);
                tracing::debug!(repository = ?path, error = %stderr, "Failed to update origin/HEAD");
            }
            Err(error) => {
                tracing::debug!(repository = ?path, %error, "Failed to run git remote set-head");
            }
        }
    }
}

/// The branch a repository is assumed to be on when nothing says otherwise.
const FALLBACK_DEFAULT_BRANCH: &str = "main";

fn fallback_default_branch() -> BranchName {
    BranchName::literal(FALLBACK_DEFAULT_BRANCH)
}

fn default_branch_of(reference: &[u8]) -> BranchName {
    String::from_utf8_lossy(reference)
        .trim()
        .strip_prefix(REMOTE_TRACKING)
        .and_then(|name| BranchName::parse(name).ok())
        .unwrap_or_else(fallback_default_branch)
}

#[cfg(test)]
mod managed_tests {
    use super::*;
    use crate::git::service::hardened::fixtures::branch;
    use crate::worktree::fixtures::git;
    use tempfile::TempDir;

    /// A host no resolver answers for, so a clone from it fails without
    /// reaching the network.
    const UNREACHABLE: &str = "https://nonexistent.invalid/repository.git";

    fn repository(path: &Path) {
        git(path, &["init", "-b", "main"]);
        std::fs::write(path.join("README.md"), "# test\n").unwrap();
        git(path, &["add", "."]);
        git(path, &["commit", "-m", "initial commit"]);
    }

    fn second_commit(path: &Path) {
        std::fs::write(path.join("file2.txt"), "second file\n").unwrap();
        git(path, &["add", "."]);
        git(path, &["commit", "-m", "second commit"]);
    }

    fn origin(path: &Path) -> String {
        format!("file://{}", path.display())
    }

    #[test]
    fn a_repository_root_is_one_with_a_git_directory_or_a_git_file() {
        let service = GitService::new();
        let temporary = TempDir::new().unwrap();
        assert!(!service.is_repository_root(temporary.path()));
        assert!(!service.is_repository_root(Path::new("/nonexistent/path")));

        std::fs::create_dir(temporary.path().join(".git")).unwrap();
        assert!(service.is_repository_root(temporary.path()));

        let worktree = TempDir::new().unwrap();
        std::fs::write(worktree.path().join(".git"), "gitdir: ../other/.git").unwrap();
        assert!(
            service.is_repository_root(worktree.path()),
            "a worktree is marked by a .git file"
        );

        let real = TempDir::new().unwrap();
        repository(real.path());
        assert!(service.is_repository_root(real.path()));
        let nested = real.path().join("subdir");
        std::fs::create_dir(&nested).unwrap();
        assert!(!service.is_repository_root(&nested));
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_git_directory_still_marks_a_repository_root() {
        let temporary = TempDir::new().unwrap();
        let real = temporary.path().join("elsewhere");
        std::fs::create_dir(&real).unwrap();
        std::os::unix::fs::symlink(&real, temporary.path().join(".git")).unwrap();

        assert!(GitService::new().is_repository_root(temporary.path()));
    }

    #[tokio::test]
    async fn the_current_branch_is_read_back_from_the_checkout() {
        let temporary = TempDir::new().unwrap();
        repository(temporary.path());
        let service = GitService::new();

        assert_eq!(
            service.current_branch(temporary.path()).await.unwrap(),
            "main"
        );

        for name in ["branch-a", "branch-b", "branch-c"] {
            git(temporary.path(), &["branch", name]);
        }
        assert_eq!(
            service.current_branch(temporary.path()).await.unwrap(),
            "main"
        );

        git(temporary.path(), &["checkout", "-q", "-b", "feature/test"]);
        assert_eq!(
            service.current_branch(temporary.path()).await.unwrap(),
            "feature/test"
        );
    }

    #[tokio::test]
    async fn a_detached_checkout_reports_head_rather_than_a_branch() {
        let temporary = TempDir::new().unwrap();
        repository(temporary.path());
        second_commit(temporary.path());
        git(temporary.path(), &["checkout", "-q", "HEAD~1"]);

        assert_eq!(
            GitService::new()
                .current_branch(temporary.path())
                .await
                .unwrap(),
            "HEAD"
        );
    }

    #[tokio::test]
    async fn a_directory_that_is_not_a_repository_has_no_current_branch() {
        let service = GitService::new();
        let temporary = TempDir::new().unwrap();

        let refusal = service
            .current_branch(temporary.path())
            .await
            .unwrap_err()
            .to_string();
        assert!(
            refusal.contains("Cannot read the repository's configuration"),
            "{refusal}"
        );

        assert!(
            service
                .current_branch(Path::new("/nonexistent/path/xyz"))
                .await
                .is_err()
        );

        let empty = TempDir::new().unwrap();
        git(empty.path(), &["init", "-q", "-b", "main"]);
        assert!(
            service.current_branch(empty.path()).await.is_err(),
            "a repository with no commits has no branch to resolve"
        );
    }

    #[tokio::test]
    async fn an_address_shaped_like_an_option_is_only_ever_an_address() {
        let service = GitService::new();
        let temporary = TempDir::new().unwrap();
        let target = temporary.path().join("repository");
        let marker = temporary.path().join("ran");

        let refusal = service
            .ensure_repository(
                &target,
                &format!("--upload-pack=touch {}", marker.display()),
                &branch("main"),
            )
            .await
            .unwrap_err()
            .to_string();

        assert!(refusal.contains("git clone failed"), "{refusal}");
        assert!(!marker.exists(), "the address was read as an option");
    }

    #[tokio::test]
    async fn an_address_that_is_merely_unreachable_is_refused_by_git_and_not_by_us() {
        let temporary = TempDir::new().unwrap();
        let target = temporary.path().join("repository");

        let refusal = GitService::new()
            .ensure_repository(&target, UNREACHABLE, &branch("main"))
            .await
            .unwrap_err()
            .to_string();

        assert!(refusal.contains("git clone failed"), "{refusal}");
    }

    #[tokio::test]
    async fn a_managed_clone_is_created_then_brought_forward_by_the_same_call() {
        let source = TempDir::new().unwrap();
        repository(source.path());
        let workspace = TempDir::new().unwrap();
        let target = workspace.path().join("cloned");
        let service = GitService::new();

        service
            .ensure_repository(&target, &origin(source.path()), &branch("main"))
            .await
            .unwrap();
        assert!(target.join(".git").exists());
        assert!(
            git(&target, &["log", "--oneline"]).contains("initial commit"),
            "the clone carries the history"
        );
        assert_eq!(service.current_branch(&target).await.unwrap(), "main");

        second_commit(source.path());
        service
            .ensure_repository(&target, &origin(source.path()), &branch("main"))
            .await
            .unwrap();
        assert!(
            git(&target, &["log", "--oneline"]).contains("second commit"),
            "the pull brought the new commit"
        );
    }

    #[tokio::test]
    async fn fetching_leaves_the_working_tree_alone_and_names_the_default_branch() {
        let source = TempDir::new().unwrap();
        repository(source.path());
        let workspace = TempDir::new().unwrap();
        let target = workspace.path().join("cloned");
        let service = GitService::new();
        let url = origin(source.path());

        service
            .ensure_repository(&target, &url, &branch("main"))
            .await
            .unwrap();
        second_commit(source.path());

        assert_eq!(
            service
                .ensure_fetched(&target, &url)
                .await
                .unwrap()
                .as_str(),
            "main"
        );
        assert!(
            git(&target, &["log", "--oneline", "origin/main"]).contains("second commit"),
            "the fetch brought the new commit into the object store"
        );
        assert!(
            !target.join("file2.txt").exists(),
            "a fetch does not advance the working tree"
        );

        assert_eq!(
            service.ensure_synced(&target, &url).await.unwrap().as_str(),
            "main"
        );
        assert!(
            target.join("file2.txt").exists(),
            "a sync does advance the working tree"
        );
    }

    #[tokio::test]
    async fn a_path_that_is_not_there_yet_is_cloned_and_one_that_is_is_fetched() {
        let service = GitService::new();
        let temporary = TempDir::new().unwrap();

        let missing = temporary.path().join("missing");
        let cloning = service
            .ensure_fetched(&missing, UNREACHABLE)
            .await
            .unwrap_err()
            .to_string();
        assert!(cloning.contains("git clone failed"), "{cloning}");

        let present = temporary.path().join("present");
        std::fs::create_dir_all(&present).unwrap();
        git(&present, &["init", "-q", "-b", "main"]);
        git(&present, &["remote", "add", ORIGIN, UNREACHABLE]);
        let fetching = service
            .ensure_fetched(&present, UNREACHABLE)
            .await
            .unwrap_err()
            .to_string();
        assert!(fetching.contains("git fetch failed"), "{fetching}");
    }

    #[tokio::test]
    async fn fetching_everything_picks_up_a_branch_the_remote_gained() {
        let source = TempDir::new().unwrap();
        repository(source.path());
        let workspace = TempDir::new().unwrap();
        let target = workspace.path().join("cloned");
        let service = GitService::new();

        service
            .ensure_repository(&target, &origin(source.path()), &branch("main"))
            .await
            .unwrap();
        git(source.path(), &["branch", "new-feature"]);
        service.fetch_all(&target).await.unwrap();

        let branches = git(&target, &["branch", "-r"]);
        assert!(branches.contains("origin/new-feature"), "{branches}");
    }

    #[tokio::test]
    async fn fetching_a_branch_needs_a_repository() {
        let service = GitService::new();
        let temporary = TempDir::new().unwrap();

        let refusal = service
            .fetch_branch(temporary.path(), &branch("main"))
            .await
            .unwrap_err()
            .to_string();
        assert!(
            refusal.contains("Cannot read the repository's configuration"),
            "{refusal}"
        );
    }

    #[tokio::test]
    async fn fetching_one_branch_brings_only_that_branch_forward() {
        let source = TempDir::new().unwrap();
        repository(source.path());
        git(source.path(), &["checkout", "-q", "-b", "feature-y"]);
        second_commit(source.path());
        git(source.path(), &["checkout", "-q", "main"]);

        let workspace = TempDir::new().unwrap();
        let target = workspace.path().join("cloned");
        let service = GitService::new();
        service
            .ensure_repository(&target, &origin(source.path()), &branch("main"))
            .await
            .unwrap();

        service
            .fetch_branch(&target, &branch("feature-y"))
            .await
            .unwrap();
        assert!(
            git(&target, &["log", "--oneline", "origin/feature-y"]).contains("second commit"),
            "the branch's commit is in the object store"
        );

        let missing = service
            .fetch_branch(&target, &branch("nonexistent-branch"))
            .await
            .unwrap_err()
            .to_string();
        assert!(missing.contains("git fetch branch failed"), "{missing}");
    }

    #[tokio::test]
    async fn a_worktree_is_added_detached_and_can_be_added_again_over_itself() {
        let temporary = TempDir::new().unwrap();
        repository(temporary.path());
        let service = GitService::new();
        let worktree = temporary.path().join("run-worktrees").join("one");

        service
            .create_worktree(temporary.path(), &worktree, &branch("main"))
            .await
            .unwrap();
        assert!(worktree.join("README.md").exists());
        assert!(
            worktree.join(".git").is_file(),
            "a worktree is marked by a .git file"
        );
        assert!(service.is_repository_root(&worktree));
        assert_eq!(service.current_branch(&worktree).await.unwrap(), "HEAD");

        service
            .create_worktree(temporary.path(), &worktree, &branch("main"))
            .await
            .expect("a worktree left by a crashed run is replaced rather than refused");
    }

    #[tokio::test]
    async fn a_worktree_refuses_a_ref_nothing_holds() {
        let service = GitService::new();
        let temporary = TempDir::new().unwrap();
        let worktree = temporary.path().join("worktree");

        assert!(
            service
                .create_worktree(temporary.path(), &worktree, &branch("main"))
                .await
                .is_err(),
            "a directory that is not a repository has no worktrees to add"
        );

        let real = TempDir::new().unwrap();
        repository(real.path());
        assert!(
            service
                .create_worktree(
                    real.path(),
                    &real.path().join("wt"),
                    &branch("nonexistent-branch")
                )
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn a_worktree_on_a_branch_is_checked_out_on_it_and_reset_to_its_start_point() {
        let temporary = TempDir::new().unwrap();
        repository(temporary.path());
        second_commit(temporary.path());
        let service = GitService::new();

        let named = temporary.path().join("named-worktrees").join("one");
        service
            .create_worktree_on_branch(
                temporary.path(),
                &named,
                &branch("my-feature"),
                &branch("main"),
            )
            .await
            .unwrap();
        assert_eq!(service.current_branch(&named).await.unwrap(), "my-feature");

        service
            .create_worktree_on_branch(
                temporary.path(),
                &named,
                &branch("my-feature"),
                &branch("main"),
            )
            .await
            .expect("a worktree left by a crashed run is replaced rather than refused");

        let first = git(temporary.path(), &["rev-parse", "HEAD~1"]);
        let earlier = temporary.path().join("earlier-worktrees").join("one");
        service
            .create_worktree_on_branch(
                temporary.path(),
                &earlier,
                &branch("earlier"),
                &branch(&first),
            )
            .await
            .unwrap();
        assert!(earlier.join("README.md").exists());
        assert!(
            !earlier.join("file2.txt").exists(),
            "the branch was reset to the start point it was given"
        );
    }

    #[tokio::test]
    async fn a_worktree_path_nested_below_its_area_is_created_on_the_way() {
        let temporary = TempDir::new().unwrap();
        repository(temporary.path());
        let service = GitService::new();

        let detached = temporary
            .path()
            .join("deep-worktrees")
            .join("nested")
            .join("one");
        service
            .create_worktree(temporary.path(), &detached, &branch("main"))
            .await
            .unwrap();
        assert!(detached.join("README.md").exists());

        let named = temporary
            .path()
            .join("deep-worktrees")
            .join("nested")
            .join("two");
        service
            .create_worktree_on_branch(
                temporary.path(),
                &named,
                &branch("new-branch"),
                &branch("main"),
            )
            .await
            .unwrap();
        assert_eq!(service.current_branch(&named).await.unwrap(), "new-branch");
    }

    #[tokio::test]
    async fn many_worktrees_of_one_repository_hold_their_own_branches() {
        let temporary = TempDir::new().unwrap();
        repository(temporary.path());
        second_commit(temporary.path());
        let service = GitService::new();
        let area = temporary.path().join("multi-worktrees");

        service
            .create_worktree_on_branch(
                temporary.path(),
                &area.join("one"),
                &branch("branch-1"),
                &branch("main"),
            )
            .await
            .unwrap();
        service
            .create_worktree_on_branch(
                temporary.path(),
                &area.join("two"),
                &branch("branch-2"),
                &branch("main"),
            )
            .await
            .unwrap();

        assert_eq!(
            service.current_branch(&area.join("one")).await.unwrap(),
            "branch-1"
        );
        assert_eq!(
            service.current_branch(&area.join("two")).await.unwrap(),
            "branch-2"
        );
        assert_eq!(
            service.current_branch(temporary.path()).await.unwrap(),
            "main"
        );
    }

    #[tokio::test]
    async fn a_worktree_is_removed_and_a_directory_that_was_never_one_is_left_alone() {
        let temporary = TempDir::new().unwrap();
        repository(temporary.path());
        let service = GitService::new();

        let area = temporary.path().join("lifecycle-worktrees");
        let worktree = area.join("one");
        service
            .create_worktree(temporary.path(), &worktree, &branch("main"))
            .await
            .unwrap();
        service
            .remove_worktree(temporary.path(), &worktree)
            .await
            .unwrap();
        assert!(!worktree.exists());

        std::fs::create_dir_all(&area).unwrap();
        service
            .remove_worktree(temporary.path(), &area.join("never-existed"))
            .await
            .expect("a worktree that is not there is already removed");

        let outside = temporary.path().join("unsafe-area").join("one");
        std::fs::create_dir_all(&outside).unwrap();
        let refusal = service
            .remove_worktree(temporary.path(), &outside)
            .await
            .unwrap_err()
            .to_string();
        assert!(refusal.contains("Refusing to remove"), "{refusal}");
        assert!(outside.exists(), "the refusal left the directory standing");
    }

    #[tokio::test]
    async fn the_default_branch_is_read_from_the_remote_head_ref_or_falls_back() {
        let source = TempDir::new().unwrap();
        repository(source.path());
        let workspace = TempDir::new().unwrap();
        let target = workspace.path().join("cloned");
        let service = GitService::new();

        service
            .ensure_repository(&target, &origin(source.path()), &branch("main"))
            .await
            .unwrap();
        git(
            &target,
            &[
                "symbolic-ref",
                "refs/remotes/origin/HEAD",
                "refs/remotes/origin/main",
            ],
        );

        assert_eq!(
            service.detect_default_branch(&target).await.as_str(),
            "main"
        );
        assert_eq!(
            service.detect_default_branch_blocking(&target).as_str(),
            "main"
        );

        let bare = TempDir::new().unwrap();
        assert_eq!(
            service.detect_default_branch(bare.path()).await.as_str(),
            "main",
            "a repository that names no default branch is assumed to use main"
        );
        assert_eq!(
            service.detect_default_branch_blocking(bare.path()).as_str(),
            "main"
        );
    }

    #[tokio::test]
    async fn a_clone_a_branch_and_a_worktree_of_it_work_together() {
        let source = TempDir::new().unwrap();
        repository(source.path());
        second_commit(source.path());

        let workspace = TempDir::new().unwrap();
        let repository_path = workspace.path().join("repository");
        let service = GitService::new();
        service
            .ensure_repository(&repository_path, &origin(source.path()), &branch("main"))
            .await
            .unwrap();
        assert_eq!(
            service.current_branch(&repository_path).await.unwrap(),
            "main"
        );

        let worktree = workspace.path().join("test-worktrees").join("fix-123");
        service
            .create_worktree_on_branch(
                &repository_path,
                &worktree,
                &branch("fix/issue-123"),
                &branch("main"),
            )
            .await
            .unwrap();

        assert_eq!(
            service.current_branch(&worktree).await.unwrap(),
            "fix/issue-123"
        );
        assert!(service.is_repository_root(&worktree));
        assert_eq!(
            service.current_branch(&repository_path).await.unwrap(),
            "main",
            "the clone stayed where it was"
        );
    }

    /// Every worktree of a managed clone shares its configuration, and a run
    /// works in one, so a key a run wrote there that no pin reaches refuses
    /// every operation that fetches into the clone, checks it out or resets
    /// it, before git runs there.
    #[tokio::test]
    async fn a_managed_clone_configured_beyond_what_git_writes_is_neither_fetched_nor_reset() {
        let source = TempDir::new().unwrap();
        repository(source.path());
        let workspace = TempDir::new().unwrap();
        let target = workspace.path().join("cloned");
        let url = origin(source.path());
        let service = GitService::new();
        service
            .ensure_repository(&target, &url, &branch("main"))
            .await
            .unwrap();
        git(&target, &["config", "alias.co", "checkout"]);

        let refusals = [
            service.fetch_all(&target).await.err(),
            service.fetch_branch(&target, &branch("main")).await.err(),
            service.pull(&target, &branch("main")).await.err(),
            service.checkout_reset(&target, &branch("main")).await.err(),
            service
                .ensure_repository(&target, &url, &branch("main"))
                .await
                .err(),
            service.ensure_fetched(&target, &url).await.err(),
            service.ensure_synced(&target, &url).await.err(),
        ];

        for (operation, refusal) in refusals.iter().enumerate() {
            assert!(
                matches!(refusal, Some(GitError::UnsafeConfig(key)) if key == "alias.co"),
                "operation {operation}: {refusal:?}"
            );
        }
    }

    /// Updating the remote's default branch is best effort, so a refused clone
    /// shows only in the ref it leaves alone.
    #[tokio::test]
    async fn the_remote_head_of_a_refused_clone_is_left_as_it_was() {
        let source = TempDir::new().unwrap();
        repository(source.path());
        let workspace = TempDir::new().unwrap();
        let target = workspace.path().join("cloned");
        let service = GitService::new();
        service
            .ensure_repository(&target, &origin(source.path()), &branch("main"))
            .await
            .unwrap();
        let head = target.join(".git").join(REMOTE_HEAD);
        git(&target, &["symbolic-ref", "--delete", REMOTE_HEAD]);
        git(&target, &["config", "alias.co", "checkout"]);

        service.update_remote_head(&target).await;
        assert!(!head.exists(), "the refused clone's remote head was set");

        git(&target, &["config", "--unset", "alias.co"]);
        service.update_remote_head(&target).await;
        assert!(head.exists(), "an accepted clone's remote head is set");
    }

    #[test]
    fn every_command_that_fetches_into_checks_out_or_resets_a_managed_clone_carries_the_pins() {
        fn arguments(command: &Command) -> Vec<String> {
            command
                .as_std()
                .get_args()
                .map(|argument| argument.to_string_lossy().into_owned())
                .collect()
        }
        fn configured_globally(command: &Command) -> bool {
            command
                .as_std()
                .get_envs()
                .any(|(key, _)| key == "GIT_CONFIG_GLOBAL")
        }
        let path = Path::new("/repository");
        let main = branch("main");
        let remote = [
            GitService::fetching_all(path),
            GitService::fetching(path, &main),
            GitService::setting_head(path),
        ];
        let local = [
            GitService::checking_out(path, &main),
            GitService::resetting(path, &main),
        ];
        let pins = PINS.map(String::from);

        for command in remote.iter().chain(&local) {
            let arguments = arguments(command);
            assert!(arguments.starts_with(&pins), "{arguments:?}");
        }
        for command in &remote {
            assert!(
                !configured_globally(command),
                "a fetch keeps the caller's own configuration: {:?}",
                arguments(command)
            );
        }
        for command in &local {
            assert!(
                configured_globally(command),
                "a checkout or reset ignores the host's configuration: {:?}",
                arguments(command)
            );
        }
    }

    /// Hooks sit in the clone's own directory, which every worktree of it
    /// shares and no check of its configuration reads, so a run can leave one
    /// there for the next fetch, checkout or reset to run as the host.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_hook_left_in_a_managed_clone_never_runs() {
        use std::os::unix::fs::PermissionsExt;

        let source = TempDir::new().unwrap();
        repository(source.path());
        let workspace = TempDir::new().unwrap();
        let target = workspace.path().join("cloned");
        let url = origin(source.path());
        let service = GitService::new();
        service
            .ensure_repository(&target, &url, &branch("main"))
            .await
            .unwrap();
        let marker = workspace.path().join("hook-ran");
        let hooks = target.join(".git").join("hooks");
        std::fs::create_dir_all(&hooks).unwrap();
        for hook in ["reference-transaction", "post-checkout"] {
            let script = hooks.join(hook);
            std::fs::write(
                &script,
                format!("#!/bin/sh\ntouch '{}'\n", marker.display()),
            )
            .unwrap();
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        second_commit(source.path());
        git(source.path(), &["branch", "feature"]);

        service.fetch_all(&target).await.unwrap();
        service
            .fetch_branch(&target, &branch("feature"))
            .await
            .unwrap();
        service
            .ensure_repository(&target, &url, &branch("main"))
            .await
            .unwrap();
        service.ensure_synced(&target, &url).await.unwrap();

        assert!(
            target.join("file2.txt").exists(),
            "the clone was brought forward"
        );
        assert!(!marker.exists(), "a hook left in the clone ran");
    }

    /// A managed command runs under the same timeout and process-group
    /// teardown as a hardened one: abandoning it takes every helper it
    /// started with it.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_managed_command_abandoned_midway_takes_its_helpers_with_it() {
        use crate::git::service::hardened::fixtures::TEARDOWN_BUDGET;
        use crate::git::service::hardened::fixtures::alive;
        use crate::git::service::hardened::fixtures::marker;

        let source = TempDir::new().unwrap();
        repository(source.path());
        let workspace = TempDir::new().unwrap();
        let target = workspace.path().join("cloned");
        GitService::new()
            .ensure_repository(&target, &origin(source.path()), &branch("main"))
            .await
            .unwrap();
        let mut command = GitService::managed_remote(&target);
        command.args([
            "-c",
            &format!(
                "remote.origin.uploadpack=/bin/sleep 60 & echo $! > '{}'; wait; true",
                workspace.path().join("helper").display()
            ),
            "fetch",
            "--",
            ORIGIN,
        ]);

        let operation = tokio::spawn(async move { GitService::output(&mut command).await });
        let helper = marker(workspace.path(), "helper").await;
        operation.abort();
        assert!(operation.await.unwrap_err().is_cancelled());
        let stopped = tokio::time::timeout(TEARDOWN_BUDGET, async {
            while alive(helper) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .is_ok();
        let _ = nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(helper as i32),
            nix::sys::signal::Signal::SIGKILL,
        );

        assert!(stopped, "a helper outlived the abandoned managed command");
    }
}
