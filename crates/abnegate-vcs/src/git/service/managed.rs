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
    /// the caller's own environment, so a local path, an SSH remote, a
    /// credential helper or a proxy the caller set up still works, over no
    /// transport but those in [`MANAGED_PROTOCOLS`].
    ///
    /// The clone's hooks and configuration are shared by every worktree of it,
    /// and a run works in one, so a hook a run left would otherwise run as the
    /// host on the next fetch. Every pin a hardened command carries is applied
    /// but the two in [`LEFT_TO_CALLER`]: a managed clone is the caller's own,
    /// and [`Self::verify_remote`] checks its configuration against the
    /// allowlist and its `origin` against the address the caller configured
    /// and the refspec git writes for a clone immediately before every fetch,
    /// so a helper or proxy the clone's configuration names, or an address or
    /// refspec a run wrote there, is refused while the caller's global ones
    /// stay usable. The hardened commands keep every pin.
    pub(super) fn managed_remote(path: &Path) -> Command {
        let mut command = Self::managed_command(Some(path));
        command.env("GIT_ALLOW_PROTOCOL", MANAGED_PROTOCOLS);
        let (pins, _) = PINS.as_chunks::<2>();
        for pin in pins
            .iter()
            .filter(|[_, setting]| !LEFT_TO_CALLER.contains(setting))
        {
            command.args(pin);
        }
        command
    }

    /// Every branch `origin` has, and none it no longer has, each forced onto
    /// its remote-tracking ref by the refspec on the command line alone: the
    /// refspecs the clone's configuration holds neither narrow nor add to
    /// what the fetch writes.
    fn fetching_all(path: &Path) -> Command {
        let mut command = Self::managed_remote(path);
        command.args([
            "fetch",
            "--prune",
            NO_FETCH_HEAD,
            IGNORE_CONFIGURED_REFSPECS,
            "--",
            ORIGIN,
            FETCH_REFSPEC,
        ]);
        command
    }

    /// One branch of `origin`, forced onto its remote-tracking ref as
    /// [`Self::fetching_all`] forces every branch.
    fn fetching(path: &Path, branch: &BranchName) -> Command {
        let mut command = Self::managed_remote(path);
        command
            .args([
                "fetch",
                NO_FETCH_HEAD,
                IGNORE_CONFIGURED_REFSPECS,
                "--",
                ORIGIN,
            ])
            .arg(tracking_refspec(branch));
        command
    }

    /// `refs/remotes/origin/HEAD` pointed at the branch `origin` reports.
    fn setting_head(path: &Path) -> Command {
        let mut command = Self::managed_remote(path);
        command.args(["remote", "set-head", "--auto", "--", ORIGIN]);
        command
    }

    /// A new managed clone of `address` at `target`, over no transport but
    /// those in [`MANAGED_PROTOCOLS`], which every later fetch into it is
    /// held to.
    fn cloning(address: &OsStr, target: &Path) -> Command {
        let mut command = Self::managed_command(None);
        command
            .env("GIT_ALLOW_PROTOCOL", MANAGED_PROTOCOLS)
            .args(["clone", "--template=", "--"])
            .arg(address)
            .arg(target);
        command
    }

    /// `branch` checked out at its remote-tracking ref, whatever it, the
    /// index and the working tree held, without recording that ref as its
    /// upstream: git would write the upstream into the clone's configuration,
    /// and through a link wherever `.git/config` is one. Unlike
    /// `reset --hard`, it writes no `ORIG_HEAD`, which git would write
    /// through a symbolic ref standing there onto whatever branch it names.
    fn checking_out(path: &Path, branch: &BranchName) -> Command {
        let mut command = Self::managed_local(path);
        command.args([
            "checkout",
            "-f",
            "--no-track",
            "-B",
            branch.as_str(),
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
    /// `default_branch`, cloning it from `url` when it is not there yet. A
    /// clone whose `origin` is no longer `url`, or no longer fetches every
    /// branch the remote has, is refused with [`GitError::UnsafeConfig`]
    /// rather than fetched.
    pub async fn ensure_repository(
        &self,
        path: &Path,
        url: &str,
        default_branch: &BranchName,
    ) -> GitResult<()> {
        match path.exists() {
            true => self.pull(path, url, default_branch).await,
            false => self.clone_managed(url, path).await,
        }
    }

    /// Ensure a managed clone's object store is current without checking out or
    /// resetting anything, and say what the remote's default branch is.
    ///
    /// `refs/remotes/origin/HEAD` is refreshed first so the answer reflects what
    /// the remote reports rather than what the clone was last told. A clone
    /// whose `origin` is no longer `url` is refused as
    /// [`Self::ensure_repository`] refuses it, and a default branch whose
    /// remote-tracking ref is a symbolic ref is refused with
    /// [`GitError::SymbolicDefaultBranch`]: following the link would name
    /// whatever branch it points at.
    pub async fn ensure_fetched(&self, path: &Path, url: &str) -> GitResult<BranchName> {
        match path.exists() {
            true => self.fetch_all(path, url).await?,
            false => self.clone_managed(url, path).await?,
        }

        self.update_remote_head(path, url).await;

        Self::default_branch(path).await
    }

    /// Ensure a managed clone is current *and* its working tree is advanced to
    /// the remote's default branch, cloning it from `url` when it is not there
    /// yet. A clone is refused as [`Self::ensure_repository`] refuses it,
    /// before anything reaches the remote or is written to the clone: one
    /// whose `origin` no longer fetches every branch the remote has is
    /// refused with [`GitError::UnsafeConfig`] rather than widened back, and
    /// its configuration, or whatever file a link there points to, is left
    /// as it was. A default branch that is a symbolic ref, or whose
    /// remote-tracking ref is one, is refused with
    /// [`GitError::SymbolicDefaultBranch`].
    pub async fn ensure_synced(&self, path: &Path, url: &str) -> GitResult<BranchName> {
        let default_branch = self.ensure_fetched(path, url).await?;
        self.checkout_reset(path, &default_branch)
            .await
            .map_err(unnamed)?;
        Ok(default_branch)
    }

    /// Refuse a managed clone before anything reaches its remote: one whose
    /// configuration [`Self::verify_config`] refuses, one whose `origin`
    /// names anything but the one address a clone of `url` is made from, or
    /// one whose `origin` fetches anything but [`FETCH_REFSPEC`]. Every
    /// worktree of the clone shares that configuration, so a run could
    /// otherwise point the next fetch at a host of its choosing, and the
    /// caller's own credential helper would be asked to answer for it, or
    /// narrow what a fetch writes so that a remote-tracking ref it set to a
    /// commit of its own survives as `origin`'s. Each listing must hold that
    /// one value alone: git reads an empty value after the address as
    /// clearing the list, and then looks for `origin` elsewhere, and an empty
    /// refspec fetches `HEAD` alone.
    async fn verify_remote(path: &Path, url: &str) -> GitResult<()> {
        Self::verify_config(path).await?;
        let expected = address(url)?;
        Self::verify_sole(path, ORIGIN_URL, expected.as_encoded_bytes()).await?;
        Self::verify_sole(path, ORIGIN_FETCH, FETCH_REFSPEC.as_bytes()).await
    }

    /// Refuse `branch` when its remote-tracking ref is a symbolic ref: a fetch
    /// naming that ref writes through the link, onto whatever ref it names.
    async fn refuse_linked_tracking(path: &Path, branch: &BranchName) -> GitResult<()> {
        match Self::is_symbolic(path, format!("{REMOTE_TRACKING}{branch}")).await? {
            true => Err(GitError::SymbolicBranch(branch.clone())),
            false => Ok(()),
        }
    }

    /// Refuse a managed clone whose own configuration gives `key` any value
    /// but `value`, no value, or that value more than once.
    async fn verify_sole(path: &Path, key: &str, value: &[u8]) -> GitResult<()> {
        let listed = Self::output(Self::managed_local(path).args(VALUE_LISTING).arg(key)).await?;
        match listed.status.success() && listed.stdout == [value, b"\0"].concat() {
            true => Ok(()),
            false => Err(GitError::UnsafeConfig(key.to_string())),
        }
    }

    /// Fetch every remote ref into a managed clone without touching its
    /// working tree. A clone whose configuration holds anything beyond what
    /// git writes for one, or whose `origin` is no longer `url` or no longer
    /// fetches every branch the remote has, is refused with
    /// [`GitError::UnsafeConfig`] first.
    pub async fn fetch_all(&self, path: &Path, url: &str) -> GitResult<()> {
        tracing::debug!(repository = ?path, "Fetching all remote refs");

        Self::verify_remote(path, url).await?;
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
    /// configured as [`Self::fetch_all`] refuses it, and a branch whose
    /// remote-tracking ref is a symbolic ref with [`GitError::SymbolicBranch`].
    pub async fn fetch_branch(&self, path: &Path, url: &str, branch: &BranchName) -> GitResult<()> {
        tracing::debug!(repository = ?path, %branch, "Fetching branch");

        Self::verify_remote(path, url).await?;
        Self::refuse_linked_tracking(path, branch).await?;
        let output = Self::output(&mut Self::fetching(path, branch)).await?;

        if !output.status.success() {
            return Err(GitError::CommandFailed(format!(
                "git fetch branch failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }

        Ok(())
    }

    /// Clone a managed repository from any address the caller can reach over
    /// a local path, HTTPS or SSH.
    async fn clone_managed(&self, url: &str, target: &Path) -> GitResult<()> {
        tracing::info!(url, target = ?target, "Cloning repository");

        if let Some(parent) = target.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let output = Self::output(&mut Self::cloning(&address(url)?, target)).await?;

        if !output.status.success() {
            return Err(GitError::CommandFailed(format!(
                "git clone failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }

        tracing::info!(target = ?target, "Repository cloned successfully");
        Ok(())
    }

    /// Fetch `branch` and advance a managed clone's working tree to it,
    /// refusing a clone configured as [`Self::fetch_all`] refuses it, and a
    /// branch [`Self::fetch_branch`] refuses.
    async fn pull(&self, path: &Path, url: &str, branch: &BranchName) -> GitResult<()> {
        tracing::debug!(repository = ?path, %branch, "Pulling latest changes");

        Self::verify_remote(path, url).await?;
        Self::refuse_linked_tracking(path, branch).await?;
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

    /// Check out `branch` at `origin/<branch>`, with the index and working
    /// tree set to it.
    ///
    /// Assumes the refs are already fetched, and discards anything the working
    /// tree or the index holds: only a managed clone may be reset this way.
    /// The checkout is local, so it runs hardened, after the clone's
    /// configuration is checked and a `branch` that is a symbolic ref, or
    /// whose remote-tracking ref is one, is refused with
    /// [`GitError::SymbolicBranch`].
    async fn checkout_reset(&self, path: &Path, branch: &BranchName) -> GitResult<()> {
        Self::verify_config(path).await?;
        if Self::is_symbolic(path, branch.reference()).await? {
            return Err(GitError::SymbolicBranch(branch.clone()));
        }
        Self::refuse_linked_tracking(path, branch).await?;
        let output = Self::output(&mut Self::checking_out(path, branch)).await?;

        if !output.status.success() {
            return Err(GitError::CommandFailed(format!(
                "git checkout failed: {}",
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
    /// `main` when the ref cannot be read or names a remote-tracking ref that
    /// is itself a symbolic ref.
    pub async fn detect_default_branch(&self, path: &Path) -> BranchName {
        Self::default_branch(path)
            .await
            .unwrap_or_else(|_| fallback_default_branch())
    }

    /// The branch `refs/remotes/origin/HEAD` names, read one link deep:
    /// following a chain through a remote-tracking ref that is itself a link
    /// would answer with whatever branch the last link names. A branch whose
    /// remote-tracking ref is a symbolic ref is refused with
    /// [`GitError::SymbolicDefaultBranch`], and one that cannot be read is
    /// `main`.
    async fn default_branch(path: &Path) -> GitResult<BranchName> {
        let output = Self::output(Self::managed_command(Some(path)).args(DEFAULT_BRANCH)).await;
        let branch = match output {
            Ok(result) if result.status.success() => default_branch_of(&result.stdout),
            _ => fallback_default_branch(),
        };
        Self::refuse_linked_tracking(path, &branch)
            .await
            .map_err(unnamed)?;
        Ok(branch)
    }

    /// [`Self::detect_default_branch`] for a caller that cannot await, such as
    /// one building a file-system index.
    pub fn detect_default_branch_blocking(&self, path: &Path) -> BranchName {
        let read = |arguments: &[&str]| {
            std::process::Command::new("git")
                .args(arguments)
                .current_dir(path)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .output()
        };
        let branch = match read(&DEFAULT_BRANCH) {
            Ok(result) if result.status.success() => default_branch_of(&result.stdout),
            _ => return fallback_default_branch(),
        };
        let tracking = format!("{REMOTE_TRACKING}{branch}");
        match read(&["symbolic-ref", "--quiet", &tracking]) {
            Ok(result) if result.status.code() == Some(NOT_SYMBOLIC) => branch,
            _ => fallback_default_branch(),
        }
    }

    /// Point `refs/remotes/origin/HEAD` at whatever the remote reports as its
    /// default branch. Best effort: it reaches the network, and a caller that
    /// cannot reach it is no worse off than before. A clone configured as
    /// [`Self::fetch_all`] refuses is left as it was.
    async fn update_remote_head(&self, path: &Path, url: &str) {
        if let Err(error) = Self::verify_remote(path, url).await {
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

/// Reads the ref `refs/remotes/origin/HEAD` names, and not the ref at the end
/// of a chain of them.
const DEFAULT_BRANCH: [&str; 3] = ["symbolic-ref", "--no-recurse", REMOTE_HEAD];

/// How `symbolic-ref --quiet` says a ref is not a symbolic ref.
const NOT_SYMBOLIC: i32 = 1;

fn fallback_default_branch() -> BranchName {
    BranchName::literal(FALLBACK_DEFAULT_BRANCH)
}

/// `error`, with a refusal of the default branch left unnamed: the
/// repository chose that name, and a refusal is read by whoever the caller
/// shows it to.
fn unnamed(error: GitError) -> GitError {
    match error {
        GitError::SymbolicBranch(_) => GitError::SymbolicDefaultBranch,
        other => other,
    }
}

/// The refspec that forces `branch` of `origin` onto its remote-tracking ref.
fn tracking_refspec(branch: &BranchName) -> String {
    format!("+{}:{REMOTE_TRACKING}{branch}", branch.reference())
}

fn default_branch_of(reference: &[u8]) -> BranchName {
    String::from_utf8_lossy(reference)
        .trim()
        .strip_prefix(REMOTE_TRACKING)
        .and_then(|name| BranchName::parse(name).ok())
        .unwrap_or_else(fallback_default_branch)
}

/// The address a managed clone of `url` is made from, and so the one its
/// `origin` must still name: a relative local path made absolute, as
/// `git clone` would record it, and every other address as given. Git reads
/// an address as a local path when it has no colon, or a slash before its
/// first one; anything else is a URL or an SSH address.
fn address(url: &str) -> GitResult<OsString> {
    let local = url.find(':').is_none_or(|colon| url[..colon].contains('/'));
    match !url.is_empty() && local && Path::new(url).is_relative() {
        true => Ok(GitService::make_absolute(Path::new(url))?.into_os_string()),
        false => Ok(OsString::from(url)),
    }
}

#[cfg(test)]
mod managed_tests {
    use super::*;
    use crate::git::service::hardened::fixtures::arguments;
    use crate::git::service::hardened::fixtures::branch;
    use crate::git::service::hardened::fixtures::recording;
    use crate::worktree::fixtures::attempt;
    use crate::worktree::fixtures::git;
    use tempfile::TempDir;

    /// A host no resolver answers for, so a clone from it fails without
    /// reaching the network.
    const UNREACHABLE: &str = "https://nonexistent.invalid/repository.git";

    /// [`UNREACHABLE`] over plain HTTP.
    const PLAIN: &str = "http://nonexistent.invalid/repository.git";

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

    /// Whether any recorded git command reached a remote.
    fn reached_remote(recorded: &[Vec<String>]) -> bool {
        recorded.iter().flatten().any(|argument| {
            matches!(
                argument.as_str(),
                "fetch" | "set-head" | "clone" | "ls-remote"
            )
        })
    }

    /// Point `refs/remotes/origin/main` at a commit only the clone at `path`
    /// has, and name that commit.
    fn track_a_local_commit(path: &Path) -> String {
        let local = git(path, &["commit-tree", "-m", "local", "HEAD^{tree}"]);
        git(path, &["update-ref", "refs/remotes/origin/main", &local]);
        local
    }

    /// What `refs/remotes/origin/main` points at in the clone at `path`.
    fn tracked(path: &Path) -> String {
        git(path, &["rev-parse", "refs/remotes/origin/main"])
    }

    /// The refspec narrowed to write `origin`'s `main` somewhere other than
    /// `refs/remotes/origin/main`.
    const NARROWED: &str = "+refs/heads/main:refs/remotes/origin/elsewhere";

    /// Whether `path`'s object store holds `object`.
    fn holds(path: &Path, object: &str) -> bool {
        std::process::Command::new("git")
            .args(["cat-file", "-e", object])
            .current_dir(path)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .status()
            .unwrap()
            .success()
    }

    /// A managed clone of `url` at `target`, made with no host
    /// configuration and its refs stored as files, whatever the host's git
    /// defaults to: the layout a test that puts a link where git keeps a ref
    /// file or directory relies on.
    fn clone_as_files(url: &str, target: &Path) {
        git(
            target.parent().unwrap(),
            &[
                "clone",
                "-q",
                "--ref-format=files",
                "--",
                url,
                target.to_str().unwrap(),
            ],
        );
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

    /// A template directory is the caller's to configure, and a link in it
    /// would be copied into the clone's git directory, where every later
    /// command refuses it; the hooks a template carries never run anyway.
    #[tokio::test]
    async fn a_managed_clone_copies_nothing_from_a_template_directory() {
        let source = TempDir::new().unwrap();
        repository(source.path());
        let workspace = TempDir::new().unwrap();
        let target = workspace.path().join("cloned");

        let (cloned, recorded) = recording(GitService::new().ensure_repository(
            &target,
            &origin(source.path()),
            &branch("main"),
        ))
        .await;

        cloned.unwrap();
        let clone = recorded
            .iter()
            .find(|command| command.iter().any(|argument| argument == "clone"))
            .expect("the repository was cloned");
        let template = clone.iter().position(|argument| argument == "--template=");
        let options = clone.iter().position(|argument| argument == "--");
        assert!(
            template.is_some_and(|template| options.is_some_and(|options| template < options)),
            "{clone:?}"
        );
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

    /// A managed clone whose default branch became a link is not brought
    /// forward: `checkout -B` would reset the branch the link names to the
    /// remote's.
    #[tokio::test]
    async fn a_managed_clone_whose_default_branch_is_a_link_is_not_brought_forward() {
        let source = TempDir::new().unwrap();
        repository(source.path());
        let workspace = TempDir::new().unwrap();
        let target = workspace.path().join("cloned");
        let service = GitService::new();
        service
            .ensure_repository(&target, &origin(source.path()), &branch("main"))
            .await
            .unwrap();
        let kept = git(
            &target,
            &["commit-tree", "-p", "HEAD", "-m", "work", "HEAD^{tree}"],
        );
        git(&target, &["update-ref", "refs/heads/task/other", &kept]);
        git(
            &target,
            &["symbolic-ref", "refs/heads/main", "refs/heads/task/other"],
        );
        second_commit(source.path());

        let synced = service
            .ensure_repository(&target, &origin(source.path()), &branch("main"))
            .await;

        assert_eq!(
            git(
                &target,
                &[
                    "for-each-ref",
                    "--format=%(objectname)",
                    "refs/heads/task/other"
                ],
            ),
            kept,
            "the branch the link names was reset to the remote's"
        );
        assert!(
            matches!(synced, Err(GitError::SymbolicBranch(ref refused)) if *refused == branch("main")),
            "{synced:?}"
        );
    }

    /// A branch whose remote-tracking ref became a link is not fetched: a
    /// fetch naming that ref writes the remote's commit through the link,
    /// onto the branch it names.
    #[tokio::test]
    async fn a_branch_whose_tracking_ref_is_a_link_is_not_fetched() {
        let source = TempDir::new().unwrap();
        repository(source.path());
        let workspace = TempDir::new().unwrap();
        let target = workspace.path().join("cloned");
        let service = GitService::new();
        service
            .ensure_repository(&target, &origin(source.path()), &branch("main"))
            .await
            .unwrap();
        let kept = git(
            &target,
            &["commit-tree", "-p", "HEAD", "-m", "work", "HEAD^{tree}"],
        );
        git(&target, &["update-ref", "refs/heads/task/other", &kept]);
        git(
            &target,
            &["update-ref", "--no-deref", "-d", "refs/remotes/origin/main"],
        );
        git(
            &target,
            &[
                "symbolic-ref",
                "refs/remotes/origin/main",
                "refs/heads/task/other",
            ],
        );
        second_commit(source.path());

        let fetched = service
            .fetch_branch(&target, &origin(source.path()), &branch("main"))
            .await;
        let pulled = service
            .ensure_repository(&target, &origin(source.path()), &branch("main"))
            .await;

        assert_eq!(
            git(
                &target,
                &[
                    "for-each-ref",
                    "--format=%(objectname)",
                    "refs/heads/task/other"
                ],
            ),
            kept,
            "the remote's commit was written onto the branch the link names"
        );
        for outcome in [fetched, pulled] {
            assert!(
                matches!(outcome, Err(GitError::SymbolicBranch(ref refused)) if *refused == branch("main")),
                "{outcome:?}"
            );
        }
    }

    /// `reset --hard` records the commit it moves from in `ORIG_HEAD`, and
    /// writes it through a symbolic ref standing there onto the branch that
    /// ref names, one another worktree may have checked out. A clone is
    /// brought forward without writing `ORIG_HEAD` at all.
    #[tokio::test]
    async fn bringing_a_clone_forward_leaves_the_branch_orig_head_names() {
        let source = TempDir::new().unwrap();
        repository(source.path());
        let workspace = TempDir::new().unwrap();
        let target = workspace.path().join("cloned");
        let url = origin(source.path());
        let service = GitService::new();
        let main = branch("main");
        service
            .ensure_repository(&target, &url, &main)
            .await
            .unwrap();
        let other = workspace.path().join("other");
        git(
            &target,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "task/other",
                other.to_str().unwrap(),
            ],
        );
        git(&other, &["commit", "-q", "--allow-empty", "-m", "work"]);
        let kept = git(&other, &["rev-parse", "HEAD"]);
        git(
            &target,
            &["symbolic-ref", "ORIG_HEAD", "refs/heads/task/other"],
        );

        for operation in 0..2 {
            git(
                source.path(),
                &[
                    "commit",
                    "-q",
                    "--allow-empty",
                    "-m",
                    &format!("advance {operation}"),
                ],
            );

            match operation {
                0 => service
                    .ensure_repository(&target, &url, &main)
                    .await
                    .unwrap(),
                _ => service
                    .ensure_synced(&target, &url)
                    .await
                    .map(drop)
                    .unwrap(),
            }

            assert_eq!(
                git(
                    &target,
                    &[
                        "for-each-ref",
                        "--format=%(objectname)",
                        "refs/heads/task/other"
                    ],
                ),
                kept,
                "operation {operation}: the branch ORIG_HEAD names was moved"
            );
            assert_eq!(
                git(&target, &["rev-parse", "HEAD"]),
                git(source.path(), &["rev-parse", "HEAD"]),
                "operation {operation}: the clone was brought forward"
            );
        }
        assert_eq!(git(&other, &["rev-parse", "HEAD"]), kept);
        assert_eq!(
            git(&target, &["symbolic-ref", "--no-recurse", "ORIG_HEAD"]),
            "refs/heads/task/other",
            "ORIG_HEAD was written"
        );
    }

    /// Bringing a clone forward discards whatever its working tree and index
    /// held: a changed file, a staged file the remote's branch does not have,
    /// and a merge left in conflict.
    #[tokio::test]
    async fn bringing_a_clone_forward_discards_a_dirty_tree_and_a_stale_index() {
        let source = TempDir::new().unwrap();
        repository(source.path());
        let workspace = TempDir::new().unwrap();
        let target = workspace.path().join("cloned");
        let url = origin(source.path());
        let service = GitService::new();
        let main = branch("main");
        service
            .ensure_repository(&target, &url, &main)
            .await
            .unwrap();
        git(&target, &["checkout", "-q", "-b", "side"]);
        std::fs::write(target.join("README.md"), "side\n").unwrap();
        git(&target, &["commit", "-q", "-am", "side"]);
        git(&target, &["checkout", "-q", "main"]);
        std::fs::write(target.join("README.md"), "mine\n").unwrap();
        git(&target, &["commit", "-q", "-am", "mine"]);
        assert!(
            !attempt(&target, &["merge", "-q", "side"]),
            "the merge is left in conflict"
        );
        std::fs::write(target.join("staged.txt"), "staged\n").unwrap();
        git(&target, &["add", "staged.txt"]);
        std::fs::write(target.join("README.md"), "changed\n").unwrap();
        second_commit(source.path());

        service
            .ensure_repository(&target, &url, &main)
            .await
            .unwrap();

        assert_eq!(
            git(&target, &["status", "--porcelain", "--untracked-files=all"]),
            ""
        );
        assert_eq!(
            git(&target, &["ls-files", "--stage"]),
            git(source.path(), &["ls-files", "--stage"]),
            "the index is the remote branch's"
        );
        assert_eq!(
            std::fs::read_to_string(target.join("README.md")).unwrap(),
            "# test\n"
        );
        assert!(!target.join("staged.txt").exists());
        assert!(!target.join(GIT_DIRECTORY).join("MERGE_HEAD").exists());
        assert_eq!(
            git(&target, &["rev-parse", "HEAD"]),
            git(source.path(), &["rev-parse", "HEAD"])
        );
    }

    /// A default branch whose remote-tracking ref is a link to another
    /// branch's reads, through `origin/HEAD` and then the link, as that other
    /// branch, and bringing it forward would discard the commits only its
    /// local branch holds. The default branch is read one link deep, and a
    /// clone is never brought forward onto a remote-tracking ref that is a
    /// link.
    #[tokio::test]
    async fn a_default_branch_whose_tracking_ref_is_a_link_is_not_synced() {
        let source = TempDir::new().unwrap();
        repository(source.path());
        git(source.path(), &["branch", "task/other"]);
        let workspace = TempDir::new().unwrap();
        let target = workspace.path().join("cloned");
        let url = origin(source.path());
        let service = GitService::new();
        let main = branch("main");
        service
            .ensure_repository(&target, &url, &main)
            .await
            .unwrap();
        let kept = git(
            &target,
            &["commit-tree", "-p", "HEAD", "-m", "work", "HEAD^{tree}"],
        );
        git(&target, &["update-ref", "refs/heads/task/other", &kept]);
        git(
            &target,
            &["update-ref", "--no-deref", "-d", "refs/remotes/origin/main"],
        );
        git(
            &target,
            &[
                "symbolic-ref",
                "refs/remotes/origin/main",
                "refs/remotes/origin/task/other",
            ],
        );
        second_commit(source.path());
        let head = git(&target, &["rev-parse", "HEAD"]);

        let synced = service.ensure_synced(&target, &url).await;
        let reset = service.checkout_reset(&target, &main).await;

        assert_eq!(
            git(
                &target,
                &[
                    "for-each-ref",
                    "--format=%(objectname)",
                    "refs/heads/task/other"
                ],
            ),
            kept,
            "the branch the link names was brought forward over its own commit"
        );
        assert_eq!(
            git(&target, &["symbolic-ref", "--no-recurse", "HEAD"]),
            "refs/heads/main"
        );
        assert_eq!(git(&target, &["rev-parse", "HEAD"]), head);
        assert!(
            matches!(synced, Err(GitError::SymbolicDefaultBranch)),
            "{synced:?}"
        );
        assert!(
            matches!(reset, Err(GitError::SymbolicBranch(ref refused)) if *refused == main),
            "{reset:?}"
        );
        assert_eq!(service.detect_default_branch(&target).await, main);
        assert_eq!(service.detect_default_branch_blocking(&target), main);
    }

    /// The default branch is named by the repository, and a name git and a
    /// [`BranchName`] both accept can hold characters that change how the
    /// text around them reads, so a refusal of it names no branch: neither
    /// one whose remote-tracking ref is a link nor one that is a link
    /// itself.
    #[tokio::test]
    async fn a_linked_default_branch_is_refused_without_the_name_the_repository_gave_it() {
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
        let name = "ma\u{200B}in";
        let tracking = format!("{REMOTE_TRACKING}{name}");
        let local = format!("{HEADS}{name}");
        git(
            &target,
            &["symbolic-ref", &tracking, "refs/remotes/origin/main"],
        );
        git(source.path(), &["branch", name]);
        git(source.path(), &["symbolic-ref", "HEAD", &local]);

        let fetched = service.ensure_fetched(&target, &url).await.map(drop);
        let synced = service.ensure_synced(&target, &url).await.map(drop);
        assert_eq!(
            git(&target, &["symbolic-ref", "--no-recurse", REMOTE_HEAD]),
            tracking,
            "the remote's default branch was not read"
        );
        git(&target, &["update-ref", "--no-deref", "-d", &tracking]);
        git(&target, &["symbolic-ref", &local, "refs/heads/main"]);
        let listed = git(
            &target,
            &[
                "for-each-ref",
                "--format=%(refname) %(objectname)",
                "refs/heads",
            ],
        );
        let reset = service.ensure_synced(&target, &url).await.map(drop);

        assert_eq!(
            git(
                &target,
                &[
                    "for-each-ref",
                    "--format=%(refname) %(objectname)",
                    "refs/heads"
                ]
            ),
            listed,
            "the refused sync moved a branch"
        );
        for (operation, refusal) in [fetched, synced, reset].into_iter().enumerate() {
            let refusal = refusal.unwrap_err();
            assert!(
                !refusal.to_string().contains(name),
                "operation {operation}: the refusal carries the name: {refusal}"
            );
            assert!(
                matches!(refusal, GitError::SymbolicDefaultBranch),
                "operation {operation}: {refusal:?}"
            );
        }
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
        let url = origin(source.path());
        let service = GitService::new();

        service
            .ensure_repository(&target, &url, &branch("main"))
            .await
            .unwrap();
        git(source.path(), &["branch", "new-feature"]);
        service.fetch_all(&target, &url).await.unwrap();

        let branches = git(&target, &["branch", "-r"]);
        assert!(branches.contains("origin/new-feature"), "{branches}");
    }

    #[tokio::test]
    async fn fetching_a_branch_needs_a_repository() {
        let service = GitService::new();
        let temporary = TempDir::new().unwrap();

        let refusal = service
            .fetch_branch(temporary.path(), UNREACHABLE, &branch("main"))
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
        let url = origin(source.path());
        let service = GitService::new();
        service
            .ensure_repository(&target, &url, &branch("main"))
            .await
            .unwrap();

        service
            .fetch_branch(&target, &url, &branch("feature-y"))
            .await
            .unwrap();
        assert!(
            git(&target, &["log", "--oneline", "origin/feature-y"]).contains("second commit"),
            "the branch's commit is in the object store"
        );

        let missing = service
            .fetch_branch(&target, &url, &branch("nonexistent-branch"))
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
            service.fetch_all(&target, &url).await.err(),
            service
                .fetch_branch(&target, &url, &branch("main"))
                .await
                .err(),
            service.pull(&target, &url, &branch("main")).await.err(),
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
        let url = origin(source.path());
        let service = GitService::new();
        service
            .ensure_repository(&target, &url, &branch("main"))
            .await
            .unwrap();
        let head = || attempt(&target, &["symbolic-ref", "--quiet", REMOTE_HEAD]);
        git(&target, &["symbolic-ref", "--delete", REMOTE_HEAD]);
        git(&target, &["config", "alias.co", "checkout"]);

        service.update_remote_head(&target, &url).await;
        assert!(!head(), "the refused clone's remote head was set");

        git(&target, &["config", "--unset", "alias.co"]);
        service.update_remote_head(&target, &url).await;
        assert!(head(), "an accepted clone's remote head is set");
    }

    /// A clone's `origin` sits in the configuration every worktree of it
    /// shares, so a run can point it at another address, and a fetch would
    /// then ask the caller's own credential helper to answer for that host.
    /// Every operation that reaches the remote refuses a clone whose `origin`
    /// is not exactly the one address the caller configured, before any git
    /// command reaches a remote.
    #[tokio::test]
    async fn a_managed_clone_whose_origin_is_not_the_configured_address_is_never_fetched() {
        let source = TempDir::new().unwrap();
        repository(source.path());
        let elsewhere = TempDir::new().unwrap();
        repository(elsewhere.path());
        second_commit(elsewhere.path());
        let unfetched = git(elsewhere.path(), &["rev-parse", "HEAD"]);
        let workspace = TempDir::new().unwrap();
        let url = origin(source.path());
        let other = origin(elsewhere.path());
        let service = GitService::new();
        let rewrites: [(&str, Vec<&str>); 6] = [
            ("replaced", vec!["config", ORIGIN_URL, &other]),
            (
                "replaced over https",
                vec!["config", ORIGIN_URL, UNREACHABLE],
            ),
            ("joined", vec!["config", "--add", ORIGIN_URL, &other]),
            ("repeated", vec!["config", "--add", ORIGIN_URL, &url]),
            ("emptied", vec!["config", "--add", ORIGIN_URL, ""]),
            ("removed", vec!["config", "--unset-all", ORIGIN_URL]),
        ];

        for (index, (rewrite, arguments)) in rewrites.iter().enumerate() {
            let target = workspace.path().join(index.to_string());
            service
                .ensure_repository(&target, &url, &branch("main"))
                .await
                .unwrap();
            git(&target, arguments);
            git(&target, &["symbolic-ref", "--delete", REMOTE_HEAD]);
            let main = branch("main");

            let refusals = [
                recording(async { service.fetch_all(&target, &url).await.err() }).await,
                recording(async { service.fetch_branch(&target, &url, &main).await.err() }).await,
                recording(async { service.pull(&target, &url, &main).await.err() }).await,
                recording(async { service.ensure_repository(&target, &url, &main).await.err() })
                    .await,
                recording(async { service.ensure_fetched(&target, &url).await.err() }).await,
                recording(async { service.ensure_synced(&target, &url).await.err() }).await,
            ];
            let ((), head) = recording(service.update_remote_head(&target, &url)).await;

            for (operation, (refusal, recorded)) in refusals.iter().enumerate() {
                assert!(
                    matches!(refusal, Some(GitError::UnsafeConfig(key)) if key == ORIGIN_URL),
                    "{rewrite}, operation {operation}: {refusal:?}"
                );
                assert!(
                    !reached_remote(recorded),
                    "{rewrite}, operation {operation}: {recorded:?}"
                );
            }
            assert!(!reached_remote(&head), "{rewrite}: {head:?}");
            assert!(
                !target.join(".git").join(REMOTE_HEAD).exists(),
                "{rewrite}: the remote head was set"
            );
            assert!(
                !holds(&target, &unfetched),
                "{rewrite}: a commit only the written address has was fetched"
            );
        }
    }

    /// The recorder that shows a refused clone is never fetched does see the
    /// fetch into one whose `origin` is still the configured address.
    #[tokio::test]
    async fn a_managed_clone_whose_origin_is_the_configured_address_is_fetched() {
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
        second_commit(source.path());
        let fetched = git(source.path(), &["rev-parse", "HEAD"]);

        let (outcome, recorded) = recording(service.ensure_fetched(&target, &url)).await;

        assert_eq!(outcome.unwrap().as_str(), "main");
        for verb in ["fetch", "set-head"] {
            assert!(
                recorded.iter().flatten().any(|argument| argument == verb),
                "{verb}: {recorded:?}"
            );
        }
        assert!(holds(&target, &fetched), "the new commit was fetched");
    }

    /// A fetch names what it fetches and where it writes it on its own
    /// command line, so the refspec a clone's configuration holds never
    /// decides which remote-tracking refs a fetch overwrites.
    #[test]
    fn every_managed_fetch_names_its_refspec_on_the_command_line() {
        let path = Path::new("/repository");
        let fetches = [
            (GitService::fetching_all(path), FETCH_REFSPEC),
            (
                GitService::fetching(path, &branch("feature/one")),
                "+refs/heads/feature/one:refs/remotes/origin/feature/one",
            ),
        ];

        for (command, refspec) in &fetches {
            let arguments = arguments(command);
            assert!(
                arguments.ends_with(&[
                    "--refmap=".to_string(),
                    "--".to_string(),
                    ORIGIN.to_string(),
                    refspec.to_string()
                ]),
                "{arguments:?}"
            );
        }
    }

    /// Every fetch each managed operation runs carries the refspec it needs
    /// on its command line.
    #[tokio::test]
    async fn every_fetch_a_managed_operation_runs_names_its_refspec() {
        let source = TempDir::new().unwrap();
        repository(source.path());
        git(source.path(), &["branch", "feature"]);
        let workspace = TempDir::new().unwrap();
        let target = workspace.path().join("cloned");
        let url = origin(source.path());
        let service = GitService::new();
        let main = branch("main");
        let feature = branch("feature");
        service
            .ensure_repository(&target, &url, &main)
            .await
            .unwrap();
        let everything = FETCH_REFSPEC.to_string();
        let one = tracking_refspec(&feature);
        let default = tracking_refspec(&main);

        let operations = [
            (
                recording(async { service.fetch_all(&target, &url).await.err() }).await,
                &everything,
            ),
            (
                recording(async { service.fetch_branch(&target, &url, &feature).await.err() })
                    .await,
                &one,
            ),
            (
                recording(async { service.pull(&target, &url, &main).await.err() }).await,
                &default,
            ),
            (
                recording(async { service.ensure_repository(&target, &url, &main).await.err() })
                    .await,
                &default,
            ),
            (
                recording(async { service.ensure_fetched(&target, &url).await.err() }).await,
                &everything,
            ),
            (
                recording(async { service.ensure_synced(&target, &url).await.err() }).await,
                &everything,
            ),
        ];

        for (operation, ((failure, recorded), refspec)) in operations.iter().enumerate() {
            assert!(failure.is_none(), "operation {operation}: {failure:?}");
            let fetches: Vec<&Vec<String>> = recorded
                .iter()
                .filter(|command| command.iter().any(|argument| argument == "fetch"))
                .collect();
            assert!(!fetches.is_empty(), "operation {operation}: {recorded:?}");
            for fetch in fetches {
                assert!(
                    fetch.iter().any(|argument| argument == NO_FETCH_HEAD),
                    "operation {operation}: {fetch:?}"
                );
                let refmap = fetch.iter().position(|argument| argument == "--refmap=");
                assert_eq!(
                    refmap.map(|refmap| &fetch[refmap + 1..]),
                    Some(["--".to_string(), ORIGIN.to_string(), refspec.to_string()].as_slice()),
                    "operation {operation}: {fetch:?}"
                );
            }
        }
        assert!(
            !target.join(GIT_DIRECTORY).join("FETCH_HEAD").exists(),
            "a fetch wrote FETCH_HEAD"
        );
    }

    /// A clone's refspec sits in the configuration every worktree of it
    /// shares, so a run can narrow it, and a remote-tracking ref it pointed
    /// at a commit of its own would then outlive the next fetch and be
    /// checked out as `origin`'s. Every operation that fetches refuses a
    /// clone whose `origin` fetches anything but every branch into its
    /// remote-tracking refs, before any git command reaches the remote, and
    /// leaves the refspec as it found it.
    #[tokio::test]
    async fn a_managed_clone_whose_refspec_is_not_the_one_git_writes_is_never_fetched() {
        let source = TempDir::new().unwrap();
        repository(source.path());
        let head = git(source.path(), &["rev-parse", "HEAD"]);
        let workspace = TempDir::new().unwrap();
        let url = origin(source.path());
        let service = GitService::new();
        let main = branch("main");
        let rewrites: [(&str, Vec<&str>); 5] = [
            ("narrowed", vec!["config", ORIGIN_FETCH, NARROWED]),
            ("joined", vec!["config", "--add", ORIGIN_FETCH, NARROWED]),
            (
                "repeated",
                vec!["config", "--add", ORIGIN_FETCH, FETCH_REFSPEC],
            ),
            ("emptied", vec!["config", "--add", ORIGIN_FETCH, ""]),
            ("removed", vec!["config", "--unset-all", ORIGIN_FETCH]),
        ];

        for (index, (rewrite, arguments)) in rewrites.iter().enumerate() {
            let target = workspace.path().join(index.to_string());
            service
                .ensure_repository(&target, &url, &main)
                .await
                .unwrap();
            let local = track_a_local_commit(&target);
            git(&target, arguments);
            git(&target, &["symbolic-ref", "--delete", REMOTE_HEAD]);
            let config = target.join(GIT_DIRECTORY).join(CONFIG_FILE);
            let written = std::fs::read_to_string(&config).unwrap();

            let refusals = [
                recording(async { service.fetch_all(&target, &url).await.err() }).await,
                recording(async { service.fetch_branch(&target, &url, &main).await.err() }).await,
                recording(async { service.pull(&target, &url, &main).await.err() }).await,
                recording(async { service.ensure_repository(&target, &url, &main).await.err() })
                    .await,
                recording(async { service.ensure_fetched(&target, &url).await.err() }).await,
                recording(async { service.ensure_synced(&target, &url).await.err() }).await,
            ];
            let ((), set_head) = recording(service.update_remote_head(&target, &url)).await;

            for (operation, (refusal, recorded)) in refusals.iter().enumerate() {
                assert!(
                    matches!(refusal, Some(GitError::UnsafeConfig(key)) if key == ORIGIN_FETCH),
                    "{rewrite}, operation {operation}: {refusal:?}"
                );
                assert!(
                    !reached_remote(recorded),
                    "{rewrite}, operation {operation}: {recorded:?}"
                );
            }
            assert!(!reached_remote(&set_head), "{rewrite}: {set_head:?}");
            assert_eq!(tracked(&target), local, "{rewrite}: nothing was fetched");
            assert_eq!(
                std::fs::read_to_string(&config).unwrap(),
                written,
                "{rewrite}: the configuration was rewritten"
            );
            assert_eq!(git(&target, &["rev-parse", "HEAD"]), head, "{rewrite}");
        }
    }

    /// A clone's `.git/config` can be a link to a file anywhere, and git
    /// writes a configuration change wherever the link points, creating the
    /// file when it is not there. A sync refuses such a clone as every other
    /// operation that fetches refuses it, and writes nothing through the link
    /// on the way.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_sync_writes_nothing_through_a_linked_configuration() {
        let source = TempDir::new().unwrap();
        repository(source.path());
        let workspace = TempDir::new().unwrap();
        let url = origin(source.path());
        let service = GitService::new();
        let main = branch("main");
        let other = workspace.path().join("other");
        service
            .ensure_repository(&other, &url, &main)
            .await
            .unwrap();
        git(&other, &["config", ORIGIN_FETCH, NARROWED]);
        let user = workspace.path().join("user");
        std::fs::write(&user, "[user]\n\tname = Fixture\n").unwrap();
        let targets = [
            ("missing", workspace.path().join("missing")),
            ("user", user),
            ("other clone", other.join(GIT_DIRECTORY).join(CONFIG_FILE)),
        ];

        for (index, (linked, target)) in targets.iter().enumerate() {
            let clone = workspace.path().join(index.to_string());
            service
                .ensure_repository(&clone, &url, &main)
                .await
                .unwrap();
            let config = clone.join(GIT_DIRECTORY).join(CONFIG_FILE);
            std::fs::remove_file(&config).unwrap();
            std::os::unix::fs::symlink(target, &config).unwrap();
            let before = std::fs::read_to_string(target).ok();

            let (refusal, recorded) = recording(service.ensure_synced(&clone, &url)).await;

            assert!(refusal.is_err(), "{linked}: {refusal:?}");
            assert!(!reached_remote(&recorded), "{linked}: {recorded:?}");
            assert_eq!(
                std::fs::read_to_string(target).ok(),
                before,
                "{linked}: the linked file was written"
            );
        }
    }

    /// A clone whose `.git/config` is a link, even to a file the
    /// configuration check accepts, is refused before it is fetched, checked
    /// out or reset. The fetch and the checkout a link made after that check
    /// would meet record no upstream for the branch, so the linked file is
    /// left byte for byte as it was either way.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_checkout_writes_nothing_through_a_linked_configuration() {
        use std::os::unix::fs::MetadataExt;

        let source = TempDir::new().unwrap();
        repository(source.path());
        let workspace = TempDir::new().unwrap();
        let target = workspace.path().join("cloned");
        let url = origin(source.path());
        let service = GitService::new();
        let main = branch("main");
        service
            .ensure_repository(&target, &url, &main)
            .await
            .unwrap();
        let config = target.join(GIT_DIRECTORY).join(CONFIG_FILE);
        let copy = workspace.path().join("copy");
        std::fs::copy(&config, &copy).unwrap();
        git(
            workspace.path(),
            &[
                "config",
                "--file",
                "copy",
                "--remove-section",
                "branch.main",
            ],
        );
        std::fs::remove_file(&config).unwrap();
        std::os::unix::fs::symlink(&copy, &config).unwrap();
        let before = std::fs::read(&copy).unwrap();
        let inode = std::fs::metadata(&copy).unwrap().ino();
        second_commit(source.path());

        let refusals = [
            service.ensure_repository(&target, &url, &main).await.err(),
            service.checkout_reset(&target, &main).await.err(),
            service.ensure_synced(&target, &url).await.err(),
        ];
        for (operation, refusal) in refusals.iter().enumerate() {
            assert!(
                matches!(refusal, Some(GitError::LinkedPath)),
                "operation {operation}: {refusal:?}"
            );
        }
        assert!(
            !target.join("file2.txt").exists(),
            "a refused clone was brought forward"
        );
        for mut command in [
            GitService::fetching(&target, &main),
            GitService::checking_out(&target, &main),
        ] {
            let output = GitService::output(&mut command).await.unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }

        assert!(
            target.join("file2.txt").exists(),
            "the clone was brought forward"
        );
        assert!(
            std::fs::symlink_metadata(&config)
                .unwrap()
                .file_type()
                .is_symlink(),
            "the link was replaced"
        );
        assert_eq!(
            String::from_utf8_lossy(&std::fs::read(&copy).unwrap()),
            String::from_utf8_lossy(&before),
            "the linked file was written"
        );
        assert_eq!(
            std::fs::metadata(&copy).unwrap().ino(),
            inode,
            "the linked file was rewritten, if with the same content"
        );
    }

    /// Every file at or below `path`, with what each holds, read without
    /// following a link below it.
    #[cfg(unix)]
    fn contents(path: &Path) -> Vec<(PathBuf, Vec<u8>)> {
        let mut found = Vec::new();
        let mut pending = vec![path.to_path_buf()];
        while let Some(next) = pending.pop() {
            match std::fs::symlink_metadata(&next).unwrap().is_dir() {
                true => pending.extend(
                    std::fs::read_dir(&next)
                        .unwrap()
                        .map(|entry| entry.unwrap().path()),
                ),
                false => {
                    let held = std::fs::read(&next).unwrap();
                    found.push((next, held));
                }
            }
        }
        found.sort();
        found
    }

    /// The [`GitError`] an error from the blocking worktree module carries.
    #[cfg(unix)]
    fn carried(error: &std::io::Error) -> Option<&GitError> {
        error
            .get_ref()
            .and_then(|inner| inner.downcast_ref::<GitError>())
    }

    /// A managed clone whose source has moved on, so bringing it forward
    /// would write its refs, reflogs, `HEAD` and working tree, with a
    /// symbolic link standing at `relative` under its git directory: to
    /// `link` when given, and otherwise to what stood there, moved out of the
    /// clone, or to a new file when nothing did. A sync and a worktree are
    /// both refused, and nothing the link points at is written.
    #[cfg(unix)]
    async fn refused_while_linked(relative: &str, link: Option<&str>) {
        let source = TempDir::new().unwrap();
        repository(source.path());
        let workspace = TempDir::new().unwrap();
        let target = workspace.path().join("cloned");
        let url = origin(source.path());
        let service = GitService::new();
        clone_as_files(&url, &target);
        let standing = target.join(GIT_DIRECTORY).join(relative);
        let pointed = match link {
            Some(link) => {
                std::fs::remove_file(&standing).unwrap();
                std::os::unix::fs::symlink(link, &standing).unwrap();
                standing.parent().unwrap().join(link)
            }
            None => {
                let moved = workspace.path().join("moved");
                match std::fs::symlink_metadata(&standing) {
                    Ok(_) => std::fs::rename(&standing, &moved).unwrap(),
                    Err(_) => std::fs::write(&moved, "planted\n").unwrap(),
                }
                std::os::unix::fs::symlink(&moved, &standing).unwrap();
                moved
            }
        };
        let before = contents(&pointed);
        second_commit(source.path());
        let worktree = workspace.path().join("worktree");

        let synced = service.ensure_synced(&target, &url).await;
        let added = crate::worktree::add(&target, &worktree, "HEAD").unwrap_err();

        assert!(
            matches!(synced, Err(GitError::LinkedPath)),
            "{relative}: {synced:?}"
        );
        assert!(
            matches!(carried(&added), Some(GitError::LinkedPath)),
            "{relative}: {added:?}"
        );
        assert_eq!(
            contents(&pointed),
            before,
            "{relative}: what the link points at was written"
        );
        assert!(
            !target.join("file2.txt").exists(),
            "{relative}: the clone was brought forward"
        );
        assert!(!worktree.exists(), "{relative}: a worktree was added");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_clone_whose_packed_refs_is_a_link_is_refused() {
        refused_while_linked("packed-refs", None).await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_clone_whose_refs_directory_is_a_link_is_refused() {
        refused_while_linked("refs", None).await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_clone_whose_branch_directory_is_a_link_is_refused() {
        refused_while_linked("refs/heads", None).await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_clone_whose_remotes_directory_is_a_link_is_refused() {
        refused_while_linked("refs/remotes", None).await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_clone_whose_origin_directory_is_a_link_is_refused() {
        refused_while_linked("refs/remotes/origin", None).await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_clone_whose_tag_directory_is_a_link_is_refused() {
        refused_while_linked("refs/tags", None).await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_clone_whose_reflog_directory_is_a_link_is_refused() {
        refused_while_linked("logs", None).await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_clone_whose_fetch_head_is_a_link_is_refused() {
        refused_while_linked("FETCH_HEAD", None).await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_clone_whose_orig_head_is_a_link_is_refused() {
        refused_while_linked("ORIG_HEAD", None).await;
    }

    /// Git reads a `HEAD` that is a link only when it points under `refs/`,
    /// as a link to a branch's own file.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_clone_whose_head_is_a_link_is_refused() {
        refused_while_linked("HEAD", Some("refs/heads/main")).await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_clone_whose_configuration_is_a_link_is_refused() {
        refused_while_linked("config", None).await;
    }

    /// Packed refs are an ordinary file of the clone's own, and a clone
    /// whose refs git has packed is brought forward and given a worktree as
    /// any other is.
    #[tokio::test]
    async fn a_clone_whose_refs_are_packed_is_brought_forward() {
        let source = TempDir::new().unwrap();
        repository(source.path());
        let workspace = TempDir::new().unwrap();
        let target = workspace.path().join("cloned");
        let url = origin(source.path());
        let service = GitService::new();
        clone_as_files(&url, &target);
        git(&target, &["pack-refs", "--all"]);
        let packed = target.join(GIT_DIRECTORY).join("packed-refs");
        assert!(std::fs::symlink_metadata(&packed).unwrap().is_file());
        second_commit(source.path());

        service.ensure_synced(&target, &url).await.unwrap();
        crate::worktree::add(&target, &workspace.path().join("worktree"), "HEAD").unwrap();

        assert!(
            target.join("file2.txt").exists(),
            "the clone was brought forward"
        );
        assert_eq!(service.current_branch(&target).await.unwrap(), "main");
        assert!(std::fs::symlink_metadata(&packed).unwrap().is_file());
    }

    /// A worktree shares its repository's git directory and keeps its own
    /// inside it, so a link in the shared one refuses a command run in the
    /// worktree, and one in the worktree's own refuses a command run in the
    /// worktree or in the repository.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_link_in_either_of_a_worktree_s_git_directories_is_refused() {
        let source = TempDir::new().unwrap();
        repository(source.path());
        let workspace = TempDir::new().unwrap();
        let target = workspace.path().join("cloned");
        let service = GitService::new();
        let main = branch("main");
        clone_as_files(&origin(source.path()), &target);
        let worktree = workspace.path().join("cloned-worktrees").join("one");
        service
            .create_worktree(&target, &worktree, &main)
            .await
            .unwrap();
        git(&target, &["pack-refs", "--all"]);
        let packed = target.join(GIT_DIRECTORY).join("packed-refs");
        let moved = workspace.path().join("moved");
        std::fs::rename(&packed, &moved).unwrap();
        std::os::unix::fs::symlink(&moved, &packed).unwrap();
        let before = contents(&moved);

        let shared = service.current_branch(&worktree).await;
        let blocking = crate::worktree::unfinished(&worktree, &[]).unwrap_err();

        assert!(matches!(shared, Err(GitError::LinkedPath)), "{shared:?}");
        assert!(
            matches!(carried(&blocking), Some(GitError::LinkedPath)),
            "{blocking:?}"
        );
        assert_eq!(contents(&moved), before);

        std::fs::remove_file(&packed).unwrap();
        std::fs::rename(&moved, &packed).unwrap();
        assert_eq!(service.current_branch(&worktree).await.unwrap(), "HEAD");
        let own = PathBuf::from(git(
            &worktree,
            &["rev-parse", "--path-format=absolute", "--git-dir"],
        ));
        let planted = workspace.path().join("planted");
        std::fs::rename(own.join("ORIG_HEAD"), &planted).unwrap();
        std::os::unix::fs::symlink(&planted, own.join("ORIG_HEAD")).unwrap();
        let held = std::fs::read(&planted).unwrap();

        let refusals = [
            service.current_branch(&worktree).await,
            service.current_branch(&target).await,
        ];

        for (operation, refusal) in refusals.iter().enumerate() {
            assert!(
                matches!(refusal, Err(GitError::LinkedPath)),
                "operation {operation}: {refusal:?}"
            );
        }
        assert_eq!(std::fs::read(&planted).unwrap(), held);
    }

    /// A remote-tracking ref pointed at a commit only the clone has is
    /// forced back to what `origin` holds by every operation that fetches
    /// it, and a pull checks out `origin`'s commit rather than that one.
    #[tokio::test]
    async fn every_managed_fetch_forces_a_remote_tracking_ref_back_to_the_remote() {
        let source = TempDir::new().unwrap();
        repository(source.path());
        let head = git(source.path(), &["rev-parse", "HEAD"]);
        let workspace = TempDir::new().unwrap();
        let target = workspace.path().join("cloned");
        let url = origin(source.path());
        let service = GitService::new();
        let main = branch("main");
        service
            .ensure_repository(&target, &url, &main)
            .await
            .unwrap();

        for operation in 0..4 {
            let local = track_a_local_commit(&target);
            assert_ne!(local, head);

            match operation {
                0 => service.fetch_all(&target, &url).await.unwrap(),
                1 => service.fetch_branch(&target, &url, &main).await.unwrap(),
                2 => service.pull(&target, &url, &main).await.unwrap(),
                _ => service
                    .ensure_fetched(&target, &url)
                    .await
                    .map(drop)
                    .unwrap(),
            }

            assert_eq!(tracked(&target), head, "operation {operation}");
            assert_eq!(
                git(&target, &["rev-parse", "HEAD"]),
                head,
                "operation {operation}"
            );
        }
    }

    /// What a fetch writes is decided by the refspec on its command line
    /// alone, so even a clone whose refspec was narrowed after it was checked
    /// has its remote-tracking ref forced back to what `origin` holds, and
    /// nothing is written where the narrowed refspec points.
    #[tokio::test]
    async fn a_fetch_forces_remote_tracking_refs_whatever_refspec_the_clone_holds() {
        let source = TempDir::new().unwrap();
        repository(source.path());
        let head = git(source.path(), &["rev-parse", "HEAD"]);
        let workspace = TempDir::new().unwrap();
        let target = workspace.path().join("cloned");
        let main = branch("main");
        GitService::new()
            .ensure_repository(&target, &origin(source.path()), &main)
            .await
            .unwrap();

        for mut command in [
            GitService::fetching_all(&target),
            GitService::fetching(&target, &main),
        ] {
            let local = track_a_local_commit(&target);
            git(&target, &["config", ORIGIN_FETCH, NARROWED]);

            let output = GitService::output(&mut command).await.unwrap();

            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert_ne!(local, head);
            assert_eq!(tracked(&target), head, "{:?}", arguments(&command));
            assert_eq!(
                git(&target, &["for-each-ref", "refs/remotes/origin/elsewhere"]),
                "",
                "{:?}",
                arguments(&command)
            );
        }
    }

    /// Git records a relative local path as an absolute one, so the address a
    /// clone is checked against is made absolute the same way, and a caller
    /// that names its source relatively can still fetch into the clone.
    #[tokio::test]
    async fn a_managed_clone_made_from_a_relative_path_is_fetched_from_it_again() {
        let source = TempDir::new().unwrap();
        repository(source.path());
        let workspace = TempDir::new().unwrap();
        let target = workspace.path().join("cloned");
        let current = std::env::current_dir().unwrap();
        let relative = current
            .components()
            .skip(1)
            .map(|_| Path::new(".."))
            .collect::<PathBuf>()
            .join(source.path().strip_prefix("/").unwrap());
        let url = relative.to_str().unwrap();
        let service = GitService::new();

        service
            .ensure_repository(&target, url, &branch("main"))
            .await
            .unwrap();
        second_commit(source.path());
        service
            .ensure_repository(&target, url, &branch("main"))
            .await
            .unwrap();

        assert!(
            target.join("file2.txt").exists(),
            "the clone was brought forward"
        );
    }

    #[test]
    fn a_relative_local_path_is_the_only_address_made_absolute() {
        let current = std::env::current_dir().unwrap();
        for unchanged in [
            "",
            "https://github.com/owner/repository.git",
            "ssh://git@host.test/owner/repository.git",
            "git@host.test:owner/repository.git",
            "file:///absolute/repository",
            "/absolute/repository",
        ] {
            assert_eq!(address(unchanged).unwrap(), unchanged, "{unchanged}");
        }
        for relative in ["repository", "../repository", "./directory:with-colon"] {
            assert_eq!(
                address(relative).unwrap(),
                current.join(relative).into_os_string(),
                "{relative}"
            );
        }
    }

    /// A managed clone is cloned and fetched over a local path, HTTPS or SSH,
    /// the transports its address can name, and never over plain HTTP.
    #[tokio::test]
    async fn a_managed_clone_is_reached_over_a_local_path_https_or_ssh_only() {
        let path = Path::new("/repository");
        let commands = [
            GitService::fetching_all(path),
            GitService::fetching(path, &branch("main")),
            GitService::setting_head(path),
            GitService::cloning(OsStr::new(UNREACHABLE), path),
        ];
        for command in &commands {
            let allowed = command
                .as_std()
                .get_envs()
                .find(|(key, _)| *key == "GIT_ALLOW_PROTOCOL")
                .and_then(|(_, value)| value);
            assert_eq!(
                allowed,
                Some(OsStr::new("file:https:ssh")),
                "{:?}",
                arguments(command)
            );
        }

        let source = TempDir::new().unwrap();
        repository(source.path());
        let workspace = TempDir::new().unwrap();
        let service = GitService::new();
        let refusal = service
            .ensure_repository(&workspace.path().join("plain"), PLAIN, &branch("main"))
            .await
            .unwrap_err()
            .to_string();
        assert!(
            refusal.contains("transport 'http' not allowed"),
            "{refusal}"
        );

        let target = workspace.path().join("cloned");
        service
            .ensure_repository(&target, &origin(source.path()), &branch("main"))
            .await
            .unwrap();
        git(&target, &["config", ORIGIN_URL, PLAIN]);
        let refusal = service
            .fetch_all(&target, PLAIN)
            .await
            .unwrap_err()
            .to_string();
        assert!(
            refusal.contains("transport 'http' not allowed"),
            "{refusal}"
        );
    }

    /// A checkout carries every pin. A fetch carries every pin but
    /// the two that would blank the caller's own credential helper and proxy,
    /// and nothing more.
    #[test]
    fn every_command_that_fetches_into_checks_out_or_resets_a_managed_clone_carries_the_pins() {
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
        let local = [GitService::checking_out(path, &main)];
        let pins = PINS.map(String::from);
        let remote_pins: Vec<String> = PINS
            .as_chunks::<2>()
            .0
            .iter()
            .filter(|[_, setting]| !matches!(*setting, "credential.helper=" | "http.proxy="))
            .flatten()
            .map(|pin| pin.to_string())
            .collect();
        assert_eq!(
            remote_pins.len(),
            PINS.len() - 4,
            "a hardened command pins both: {PINS:?}"
        );

        for command in &remote {
            let arguments = arguments(command);
            assert!(arguments.starts_with(&remote_pins), "{arguments:?}");
            assert_ne!(
                arguments[remote_pins.len()],
                "-c",
                "no pin beyond that set: {arguments:?}"
            );
            assert!(
                arguments.contains(&"http.followRedirects=false".to_string()),
                "{arguments:?}"
            );
            assert!(
                !configured_globally(command),
                "a fetch keeps the caller's own configuration: {arguments:?}"
            );
        }
        for command in &local {
            let arguments = arguments(command);
            assert!(arguments.starts_with(&pins), "{arguments:?}");
            assert!(
                configured_globally(command),
                "a checkout ignores the host's configuration: {arguments:?}"
            );
        }
    }

    /// A managed clone is the caller's own, so a fetch into it keeps the
    /// credential helper and proxy the caller configured globally rather than
    /// blanking them with a pin. Both are resolved under exactly the options
    /// `fetch_all` runs git with.
    #[test]
    fn a_fetch_into_a_managed_clone_keeps_the_caller_s_credential_helper_and_proxy() {
        let clone = TempDir::new().unwrap();
        repository(clone.path());
        let caller = TempDir::new().unwrap();
        let global = caller.path().join("gitconfig");
        std::fs::write(
            &global,
            "[credential]\n\thelper = fixture-helper\n[http]\n\tproxy = http://proxy.test:3128\n",
        )
        .unwrap();
        let fetch = arguments(&GitService::fetching_all(clone.path()));
        let options = &fetch[..fetch
            .iter()
            .position(|argument| argument == "fetch")
            .unwrap()];

        for blanking in ["credential.helper=", "http.proxy="] {
            assert!(
                !options.contains(&blanking.to_string()),
                "{blanking}: {options:?}"
            );
        }
        for (key, value) in [
            ("credential.helper", "fixture-helper"),
            ("http.proxy", "http://proxy.test:3128"),
        ] {
            let resolved = std::process::Command::new("git")
                .args(options)
                .args(["config", "--get-all", key])
                .env("GIT_CONFIG_GLOBAL", &global)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .current_dir(clone.path())
                .output()
                .unwrap();
            assert_eq!(
                String::from_utf8_lossy(&resolved.stdout),
                format!("{value}\n"),
                "{key}"
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

        service.fetch_all(&target, &url).await.unwrap();
        service
            .fetch_branch(&target, &url, &branch("feature"))
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
