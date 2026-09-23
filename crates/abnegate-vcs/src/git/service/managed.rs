use super::*;

impl GitService {
    /// A git invocation against a clone the caller owns outright, run with the
    /// caller's own environment so an address only that environment can reach —
    /// a local path, an SSH remote, a credential helper — still works.
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
        default_branch: &str,
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
    pub async fn ensure_fetched(&self, path: &Path, url: &str) -> GitResult<String> {
        match path.exists() {
            true => self.fetch_all(path).await?,
            false => self.clone_managed(url, path).await?,
        }

        self.update_remote_head(path).await;

        Ok(self.detect_default_branch(path).await)
    }

    /// Ensure a managed clone is current *and* its working tree is advanced to
    /// the remote's default branch.
    pub async fn ensure_synced(&self, path: &Path, url: &str) -> GitResult<String> {
        // Repair single-branch or stale-refspec clones so the fetch sees the
        // current default branch.
        if self.is_repository_root(path) {
            self.track_all_branches(path).await;
        }
        let default_branch = self.ensure_fetched(path, url).await?;
        self.checkout_reset(path, &default_branch).await?;
        Ok(default_branch)
    }

    /// Widen the fetch refspec to every branch the remote has.
    async fn track_all_branches(&self, path: &Path) {
        let output = Self::managed_command(Some(path))
            .args(["remote", "set-branches", "origin", "*"])
            .output()
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
    /// working tree.
    pub async fn fetch_all(&self, path: &Path) -> GitResult<()> {
        tracing::debug!(repository = ?path, "Fetching all remote refs");

        let output = Self::managed_command(Some(path))
            .args(["fetch", "origin", "--prune"])
            .output()
            .await?;

        if !output.status.success() {
            return Err(GitError::CommandFailed(format!(
                "git fetch failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }

        tracing::debug!(repository = ?path, "Fetch completed");
        Ok(())
    }

    /// Fetch one branch from `origin` into a managed clone.
    pub async fn fetch_branch(&self, path: &Path, branch: &str) -> GitResult<()> {
        validate_reference(branch, "branch name")?;

        tracing::debug!(repository = ?path, branch, "Fetching branch");

        let output = Self::managed_command(Some(path))
            .args(["fetch", "origin", branch])
            .output()
            .await?;

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

        // An address starting with `-` is read by git as an option, and the
        // shell metacharacters are refused so a URL from a configuration file
        // cannot become one.
        if url.starts_with('-')
            || url.contains(';')
            || url.contains('|')
            || url.contains('$')
            || url.contains('`')
        {
            return Err(GitError::InvalidReference {
                label: "repository URL".to_string(),
                value: url.to_string(),
            });
        }

        let output = Self::managed_command(None)
            .args(["clone", "--", url])
            .arg(target)
            .output()
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
    async fn pull(&self, path: &Path, branch: &str) -> GitResult<()> {
        tracing::debug!(repository = ?path, branch, "Pulling latest changes");

        validate_reference(branch, "branch name")?;

        let output = Self::managed_command(Some(path))
            .args(["fetch", "origin", branch])
            .output()
            .await?;

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
    /// tree holds: only a managed clone may be reset this way.
    async fn checkout_reset(&self, path: &Path, branch: &str) -> GitResult<()> {
        validate_reference(branch, "branch name")?;

        let remote = format!("origin/{branch}");

        let output = Self::managed_command(Some(path))
            .args(["checkout", "-f", "-B", branch, &remote])
            .output()
            .await?;

        if !output.status.success() {
            return Err(GitError::CommandFailed(format!(
                "git checkout failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }

        let output = Self::managed_command(Some(path))
            .args(["reset", "--hard", &remote])
            .output()
            .await?;

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
    pub async fn detect_default_branch(&self, path: &Path) -> String {
        let output = Self::managed_command(Some(path))
            .args(["symbolic-ref", "refs/remotes/origin/HEAD"])
            .output()
            .await;

        match output {
            Ok(result) if result.status.success() => default_branch_of(&result.stdout),
            _ => FALLBACK_DEFAULT_BRANCH.to_string(),
        }
    }

    /// [`Self::detect_default_branch`] for a caller that cannot await, such as
    /// one building a file-system index.
    pub fn detect_default_branch_blocking(&self, path: &Path) -> String {
        let output = std::process::Command::new("git")
            .args(["symbolic-ref", "refs/remotes/origin/HEAD"])
            .current_dir(path)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output();

        match output {
            Ok(result) if result.status.success() => default_branch_of(&result.stdout),
            _ => FALLBACK_DEFAULT_BRANCH.to_string(),
        }
    }

    /// Point `refs/remotes/origin/HEAD` at whatever the remote reports as its
    /// default branch. Best effort: it reaches the network, and a caller that
    /// cannot reach it is no worse off than before.
    async fn update_remote_head(&self, path: &Path) {
        let output = Self::managed_command(Some(path))
            .args(["remote", "set-head", "origin", "--auto"])
            .output()
            .await;

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

fn default_branch_of(reference: &[u8]) -> String {
    String::from_utf8_lossy(reference)
        .trim()
        .strip_prefix("refs/remotes/origin/")
        .unwrap_or(FALLBACK_DEFAULT_BRANCH)
        .to_string()
}

/// Refuse a ref name that git would read as an option or a revision expression.
pub(super) fn validate_reference(name: &str, label: &str) -> GitResult<()> {
    let refused = name.is_empty()
        || name == "@"
        || name.starts_with('-')
        || name.contains("..")
        || !name.chars().all(|character| {
            character.is_alphanumeric() || matches!(character, '-' | '_' | '.' | '/' | '@')
        });

    match refused {
        true => Err(GitError::InvalidReference {
            label: label.to_string(),
            value: name.to_string(),
        }),
        false => Ok(()),
    }
}

#[cfg(test)]
mod reference_tests {
    use super::*;

    #[test]
    fn a_ref_of_letters_digits_and_separators_is_accepted() {
        for name in [
            "main",
            "develop",
            "feature/my-thing",
            "release/v1.2.3",
            "feature_branch",
            "user/feature.name",
            "user@feature",
            "a@b@c",
            "12345",
            "a",
            "1",
            "Feature/MyBranch",
            "UPPERCASE",
            "v1.0.0",
            "release.1.2.3",
            ".",
            ".branch",
            "br\u{00e4}nch",
            "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-_./a@b",
        ] {
            assert!(validate_reference(name, "ref").is_ok(), "{name:?}");
        }
        assert!(validate_reference(&"a".repeat(256), "branch").is_ok());
    }

    #[test]
    fn a_ref_that_is_an_option_a_revision_or_a_shell_word_is_refused() {
        for name in [
            "",
            "@",
            "-evil",
            "--evil",
            "main..evil",
            "..",
            "a...b",
            "..branch",
            "branch..",
            "$(whoami)",
            "`id`",
            "a;b",
            "a|b",
            "a&b",
            "a>b",
            "a<b",
            "a b",
            "a\tb",
            "a\nb",
            "a\rb",
            "a'b",
            "a\"b",
            "a!b",
            "a#b",
            "a%b",
            "a(b)",
            "a{b}",
            "a=b",
            "a+b",
            "a,b",
            "$HOME",
            "HEAD~1",
            "HEAD^",
            "refs:heads",
            "branch?",
            "branch*",
            "branch[0]",
            "stash@{0}",
            "path\\name",
            "main\0evil",
            "branch\u{200b}name",
            "\u{2026}",
            "branch\u{1}name",
            "branch\u{7f}",
        ] {
            assert!(validate_reference(name, "ref").is_err(), "{name:?}");
        }
    }

    #[test]
    fn a_refusal_names_both_what_was_expected_and_what_arrived() {
        let refusal = validate_reference("--evil", "checkout ref")
            .unwrap_err()
            .to_string();

        assert!(refusal.contains("checkout ref"), "{refusal}");
        assert!(refusal.contains("--evil"), "{refusal}");
        assert!(refusal.contains("disallowed characters"), "{refusal}");
    }
}

#[cfg(test)]
mod managed_tests {
    use super::*;
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
        assert!(refusal.contains("git rev-parse failed"), "{refusal}");

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
    async fn an_address_git_would_read_as_an_option_or_a_shell_word_is_refused() {
        let service = GitService::new();
        let temporary = TempDir::new().unwrap();
        let target = temporary.path().join("repository");

        for url in [
            "--upload-pack=evil",
            "https://example.com;rm -rf /",
            "https://example.com|evil",
            "https://example.com/$HOME",
            "https://example.com/`evil`",
        ] {
            let refusal = service
                .ensure_repository(&target, url, "main")
                .await
                .unwrap_err()
                .to_string();
            assert!(
                refusal.contains("disallowed characters"),
                "{url}: {refusal}"
            );
        }
    }

    #[tokio::test]
    async fn an_address_that_is_merely_unreachable_is_refused_by_git_and_not_by_us() {
        let temporary = TempDir::new().unwrap();
        let target = temporary.path().join("repository");

        let refusal = GitService::new()
            .ensure_repository(&target, UNREACHABLE, "main")
            .await
            .unwrap_err()
            .to_string();

        assert!(refusal.contains("git clone failed"), "{refusal}");
    }

    #[tokio::test]
    async fn a_branch_name_is_validated_before_a_managed_clone_is_pulled() {
        let service = GitService::new();
        let temporary = TempDir::new().unwrap();
        let target = temporary.path().join("repository");
        std::fs::create_dir_all(&target).unwrap();

        for branch in ["--evil-option", "main;evil", "main..evil", "", "@"] {
            let refusal = service
                .ensure_repository(&target, "https://example.com/repository.git", branch)
                .await
                .unwrap_err()
                .to_string();
            assert!(
                refusal.contains("disallowed"),
                "{branch:?} was not refused: {refusal}"
            );
        }
    }

    #[tokio::test]
    async fn a_managed_clone_is_created_then_brought_forward_by_the_same_call() {
        let source = TempDir::new().unwrap();
        repository(source.path());
        let workspace = TempDir::new().unwrap();
        let target = workspace.path().join("cloned");
        let service = GitService::new();

        service
            .ensure_repository(&target, &origin(source.path()), "main")
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
            .ensure_repository(&target, &origin(source.path()), "main")
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
            .ensure_repository(&target, &url, "main")
            .await
            .unwrap();
        second_commit(source.path());

        assert_eq!(service.ensure_fetched(&target, &url).await.unwrap(), "main");
        assert!(
            git(&target, &["log", "--oneline", "origin/main"]).contains("second commit"),
            "the fetch brought the new commit into the object store"
        );
        assert!(
            !target.join("file2.txt").exists(),
            "a fetch does not advance the working tree"
        );

        assert_eq!(service.ensure_synced(&target, &url).await.unwrap(), "main");
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
            .ensure_repository(&target, &origin(source.path()), "main")
            .await
            .unwrap();
        git(source.path(), &["branch", "new-feature"]);
        service.fetch_all(&target).await.unwrap();

        let branches = git(&target, &["branch", "-r"]);
        assert!(branches.contains("origin/new-feature"), "{branches}");
    }

    #[tokio::test]
    async fn fetching_a_branch_validates_its_name_and_needs_a_repository() {
        let service = GitService::new();
        let temporary = TempDir::new().unwrap();

        for branch in ["--evil", "", "a..b"] {
            assert!(
                service
                    .fetch_branch(temporary.path(), branch)
                    .await
                    .is_err(),
                "{branch:?}"
            );
        }

        let refusal = service
            .fetch_branch(temporary.path(), "main")
            .await
            .unwrap_err()
            .to_string();
        assert!(refusal.contains("git fetch branch failed"), "{refusal}");
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
            .ensure_repository(&target, &origin(source.path()), "main")
            .await
            .unwrap();

        service.fetch_branch(&target, "feature-y").await.unwrap();
        assert!(
            git(&target, &["log", "--oneline", "origin/feature-y"]).contains("second commit"),
            "the branch's commit is in the object store"
        );

        let missing = service
            .fetch_branch(&target, "nonexistent-branch")
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
            .create_worktree(temporary.path(), &worktree, "main")
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
            .create_worktree(temporary.path(), &worktree, "main")
            .await
            .expect("a worktree left by a crashed run is replaced rather than refused");
    }

    #[tokio::test]
    async fn a_worktree_refuses_a_ref_that_is_an_option_and_a_ref_nothing_holds() {
        let service = GitService::new();
        let temporary = TempDir::new().unwrap();
        let worktree = temporary.path().join("worktree");

        for reference in ["--evil-ref", ""] {
            let refusal = service
                .create_worktree(temporary.path(), &worktree, reference)
                .await
                .unwrap_err()
                .to_string();
            assert!(refusal.contains("disallowed characters"), "{refusal}");
        }

        assert!(
            service
                .create_worktree(temporary.path(), &worktree, "main")
                .await
                .is_err(),
            "a directory that is not a repository has no worktrees to add"
        );

        let real = TempDir::new().unwrap();
        repository(real.path());
        assert!(
            service
                .create_worktree(real.path(), &real.path().join("wt"), "nonexistent-branch")
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
            .create_worktree_on_branch(temporary.path(), &named, "my-feature", "main")
            .await
            .unwrap();
        assert_eq!(service.current_branch(&named).await.unwrap(), "my-feature");

        service
            .create_worktree_on_branch(temporary.path(), &named, "my-feature", "main")
            .await
            .expect("a worktree left by a crashed run is replaced rather than refused");

        let first = git(temporary.path(), &["rev-parse", "HEAD~1"]);
        let earlier = temporary.path().join("earlier-worktrees").join("one");
        service
            .create_worktree_on_branch(temporary.path(), &earlier, "earlier", &first)
            .await
            .unwrap();
        assert!(earlier.join("README.md").exists());
        assert!(
            !earlier.join("file2.txt").exists(),
            "the branch was reset to the start point it was given"
        );
    }

    #[tokio::test]
    async fn a_worktree_on_a_branch_validates_both_names_it_is_given() {
        let service = GitService::new();
        let temporary = TempDir::new().unwrap();
        let worktree = temporary.path().join("worktree");

        for (branch, start) in [
            ("--evil", "main"),
            ("my-branch", "--evil"),
            ("a..b", "main"),
            ("branch", "a..b"),
        ] {
            let refusal = service
                .create_worktree_on_branch(temporary.path(), &worktree, branch, start)
                .await
                .unwrap_err()
                .to_string();
            assert!(
                refusal.contains("disallowed characters"),
                "{branch:?}/{start:?}: {refusal}"
            );
        }
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
            .create_worktree(temporary.path(), &detached, "main")
            .await
            .unwrap();
        assert!(detached.join("README.md").exists());

        let named = temporary
            .path()
            .join("deep-worktrees")
            .join("nested")
            .join("two");
        service
            .create_worktree_on_branch(temporary.path(), &named, "new-branch", "main")
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
            .create_worktree_on_branch(temporary.path(), &area.join("one"), "branch-1", "main")
            .await
            .unwrap();
        service
            .create_worktree_on_branch(temporary.path(), &area.join("two"), "branch-2", "main")
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
            .create_worktree(temporary.path(), &worktree, "main")
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
            .ensure_repository(&target, &origin(source.path()), "main")
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

        assert_eq!(service.detect_default_branch(&target).await, "main");
        assert_eq!(service.detect_default_branch_blocking(&target), "main");

        let bare = TempDir::new().unwrap();
        assert_eq!(
            service.detect_default_branch(bare.path()).await,
            "main",
            "a repository that names no default branch is assumed to use main"
        );
        assert_eq!(service.detect_default_branch_blocking(bare.path()), "main");
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
            .ensure_repository(&repository_path, &origin(source.path()), "main")
            .await
            .unwrap();
        assert_eq!(
            service.current_branch(&repository_path).await.unwrap(),
            "main"
        );

        let worktree = workspace.path().join("test-worktrees").join("fix-123");
        service
            .create_worktree_on_branch(&repository_path, &worktree, "fix/issue-123", "main")
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
}
