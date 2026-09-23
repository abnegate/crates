//! Git operations, run by shelling out to `git`.
//!
//! [`GitService`] covers two kinds of repository. A *hardened* one is reached
//! over HTTPS at github.com, cloned and pushed with a credential that never
//! enters the URL, the process table or `.git/config`, and run with every
//! setting a repository could name a program through pinned on the command
//! line. A *managed* one is a local clone the caller owns outright: it is
//! cloned, fetched, reset and given worktrees from whatever address the caller
//! configured, including a local path.

use base64::Engine;
use std::path::Path;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;
use thiserror::Error;
use tokio::process::Command;
use url::Url;
use uuid::Uuid;

/// Longest diff kept before truncation, in bytes.
const MAXIMUM_DIFF_BYTES: usize = 50_000;

/// Longest a generated branch name may run.
const MAXIMUM_BRANCH_LENGTH: usize = 100;

/// Longest a hardened git command may run before it is torn down.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(300);

/// The branch namespace [`GitService::generate_branch_name`] works in.
const DEFAULT_BRANCH_PREFIX: &str = "task";

/// The author a commit is made under when the caller names nobody.
const DEFAULT_AUTHOR_NAME: &str = "abnegate-vcs";

/// The address a commit is made under when the caller names nobody.
const DEFAULT_AUTHOR_EMAIL: &str = "abnegate-vcs@localhost";

/// The suffix a directory holding worktrees has to carry before
/// [`GitService::remove_worktree`] will delete anything inside it by hand.
const WORKTREE_AREA_SUFFIX: &str = "-worktrees";

/// What went wrong running git.
#[derive(Debug, Error)]
pub enum GitError {
    #[error("Git command failed: {0}")]
    CommandFailed(String),

    #[error("Repository not found at {0}")]
    RepositoryNotFound(String),

    #[error("Remote not configured")]
    NoRemote,

    #[error("No changes to commit")]
    NoChanges,

    #[error("Branch already exists: {0}")]
    BranchExists(String),

    #[error("Authentication failed")]
    AuthenticationFailed,

    #[error("Invalid {label} (contains disallowed characters): {value}")]
    InvalidReference { label: String, value: String },

    #[error("Refusing to remove directory outside worktrees area: {0}")]
    UnsafeWorktree(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

pub type GitResult<T> = Result<T, GitError>;

/// What a working tree currently differs from its last commit by.
#[derive(Debug, Clone)]
pub struct DiffSummary {
    pub files_changed: Vec<String>,
    pub insertions: u32,
    pub deletions: u32,
    pub diff_text: String,
}

/// A remote's default branch and the commit at its tip, as the remote itself
/// reports them: what a run starts from is asked of the repository, not read
/// from a ref every run of it shares.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteHead {
    pub branch: String,
    pub commit: String,
}

/// Git operations against a repository on disk.
#[derive(Debug, Clone)]
pub struct GitService {
    max_branch_length: usize,
    branch_prefix: String,
    author_name: String,
    author_email: String,
}

impl Default for GitService {
    fn default() -> Self {
        Self::new()
    }
}

/// The group remains separate from the server so cancellation cannot signal
/// another task. Drop covers timeout/future cancellation, not server SIGKILL;
/// abrupt process death requires the hosting supervisor to tear down its group.
#[cfg(unix)]
struct Group(nix::unistd::Pid);

#[cfg(unix)]
impl Drop for Group {
    fn drop(&mut self) {
        if let Err(error) = nix::sys::signal::killpg(self.0, nix::sys::signal::Signal::SIGKILL)
            && error != nix::errno::Errno::ESRCH
        {
            tracing::warn!(%error, group = self.0.as_raw(), "Could not terminate Git process group");
        }
    }
}

impl GitService {
    pub fn new() -> Self {
        Self {
            max_branch_length: MAXIMUM_BRANCH_LENGTH,
            branch_prefix: DEFAULT_BRANCH_PREFIX.to_string(),
            author_name: DEFAULT_AUTHOR_NAME.to_string(),
            author_email: DEFAULT_AUTHOR_EMAIL.to_string(),
        }
    }

    /// Name generated branches under `prefix` rather than `task`.
    #[must_use]
    pub fn with_branch_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.branch_prefix = prefix.into();
        self
    }

    /// Commit as `name <email>` rather than as the crate.
    #[must_use]
    pub fn with_author(mut self, name: impl Into<String>, email: impl Into<String>) -> Self {
        self.author_name = name.into();
        self.author_email = email.into();
        self
    }

    /// Cut generated branch names at `length` characters.
    #[must_use]
    pub fn with_max_branch_length(mut self, length: usize) -> Self {
        self.max_branch_length = length;
        self
    }

    /// Accept GitHub HTTPS repositories without URL credentials or transport options.
    pub fn repository_url(source: &str) -> GitResult<String> {
        let invalid =
            || GitError::CommandFailed("Expected an HTTPS GitHub owner/repository URL".to_string());
        let url = Url::parse(source).map_err(|_| invalid())?;
        if url.scheme() != "https"
            || url.host_str() != Some("github.com")
            || !url.username().is_empty()
            || url.password().is_some()
            || url.port().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(invalid());
        }
        let path = url.path().trim_matches('/');
        let parts: Vec<&str> = path.split('/').collect();
        if parts.len() != 2
            || parts.iter().any(|part| {
                part.is_empty()
                    || *part == "."
                    || *part == ".."
                    || !part.chars().all(|character| {
                        character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
                    })
            })
        {
            return Err(invalid());
        }
        let name = parts[1].strip_suffix(".git").unwrap_or(parts[1]);
        if name.is_empty() {
            return Err(invalid());
        }
        Ok(format!("https://github.com/{}/{name}.git", parts[0]))
    }

    /// Clone into an empty, caller-owned directory. Credentials live only in the
    /// child environment, never the origin URL, process arguments, or git config.
    pub async fn clone_repository(
        &self,
        source: &str,
        destination: &Path,
        token: Option<&str>,
    ) -> GitResult<()> {
        let source = Self::repository_url(source)?;
        self.clone_source(&source, destination, token, false).await
    }

    fn network_command(token: Option<&str>) -> Command {
        // Task-local replacement refs and legacy grafts must not reinterpret
        // the stored objects used by history checks, diffs, commits or pushes.
        // Nor may the repository's configuration name a program for git to
        // run: a task's own git commands can write that configuration, and it
        // is shared by every worktree of the base clone, so the hook path, the
        // file-system monitor and commit signing are pinned on the command
        // line, where the configuration cannot reach.
        let mut command = Command::new("git");
        command
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_NO_REPLACE_OBJECTS", "1")
            .env("GIT_GRAFT_FILE", "/dev/null")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_ASKPASS", "/usr/bin/false")
            .env("GIT_ALLOW_PROTOCOL", "https")
            .args([
                "-c",
                "credential.helper=",
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "core.fsmonitor=false",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "tag.gpgsign=false",
                "-c",
                "http.followRedirects=false",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        if let Some(token) = token {
            authenticate(&mut command, token);
        }
        command
    }

    async fn output(command: &mut Command) -> GitResult<std::process::Output> {
        #[cfg(unix)]
        command.process_group(0);
        command.kill_on_drop(true);
        let child = command.spawn()?;
        #[cfg(unix)]
        let _group = Group(nix::unistd::Pid::from_raw(
            child
                .id()
                .ok_or_else(|| std::io::Error::other("Git process has no ID"))? as i32,
        ));
        match tokio::time::timeout(COMMAND_TIMEOUT, child.wait_with_output()).await {
            Ok(output) => Ok(output?),
            Err(_) => Err(GitError::CommandFailed(
                "Git operation timed out".to_string(),
            )),
        }
    }

    async fn finish(command: &mut Command) -> GitResult<()> {
        let output = Self::output(command).await?;
        if !output.status.success() {
            return Err(GitError::CommandFailed(
                "Git operation failed; verify repository access".to_string(),
            ));
        }
        Ok(())
    }

    async fn clone_source(
        &self,
        source: &str,
        destination: &Path,
        token: Option<&str>,
        local: bool,
    ) -> GitResult<()> {
        let mut command = Self::network_command(token);
        // Local transport is reachable only from this module's controlled fixture tests.
        if local {
            #[cfg(test)]
            command.env("GIT_ALLOW_PROTOCOL", "file");
            #[cfg(not(test))]
            return Err(GitError::CommandFailed(
                "Local repositories are disabled".to_string(),
            ));
        }
        command
            .args(["clone", "--no-hardlinks", "--template=", "--", source])
            .arg(destination);
        Self::finish(&mut command).await
    }

    /// Resume the task branch from a fresh clone without rewriting its history.
    pub async fn prepare_branch(&self, path: &Path, branch: &str, required: bool) -> GitResult<()> {
        let reference = format!("refs/heads/{branch}");
        let valid =
            Self::output(Self::network_command(None).args(["check-ref-format", &reference]))
                .await?;
        if !valid.status.success() || branch.starts_with('-') {
            return Err(GitError::CommandFailed("Invalid task branch".into()));
        }
        let remote = format!("refs/remotes/origin/{branch}");
        let exists = Self::output(
            Self::network_command(None)
                .args(["show-ref", "--verify", "--quiet", &remote])
                .current_dir(path),
        )
        .await?;
        if !exists.status.success() && (required || exists.status.code() != Some(1)) {
            return Err(GitError::CommandFailed(
                "Task branch is missing from the repository".into(),
            ));
        }
        if self.current_branch(path).await? == branch {
            return Ok(());
        }
        // A run works in a worktree of a base clone the repository's runs
        // share, so a branch that already exists here is either one an earlier
        // run's worktree still holds — kept because it has work no remote has,
        // and the run is refused with that reason rather than git's wording —
        // or one nothing holds: left by a removal whose deletion failed, or by
        // a kept worktree deleted by hand. That one may still point at commits
        // nothing else has, so it is set aside under a name of its own rather
        // than deleted, and the run takes the name.
        let held = Self::output(
            Self::network_command(None)
                .args(["show-ref", "--verify", "--quiet", &reference])
                .current_dir(path),
        )
        .await?;
        if held.status.success() {
            // A worktree whose directory was deleted by hand no longer holds
            // anything; prune it so it cannot stand in for one that does.
            let mut prune = Self::network_command(None);
            prune.args(["worktree", "prune"]).current_dir(path);
            let _ = Self::finish(&mut prune).await;
            let listed = Self::output(
                Self::network_command(None)
                    .args(["worktree", "list", "--porcelain"])
                    .current_dir(path)
                    .stdout(Stdio::piped()),
            )
            .await?;
            let holder = format!("branch {reference}");
            if String::from_utf8_lossy(&listed.stdout)
                .lines()
                .any(|line| line == holder)
            {
                return Err(GitError::CommandFailed(
                    "An earlier run's worktree still holds this task's branch with work that was \
                     never published; publish or remove it before running the task again"
                        .into(),
                ));
            }
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|since| format!("{}.{:09}", since.as_secs(), since.subsec_nanos()))
                .unwrap_or_else(|_| "0".to_string());
            let aside = format!("{branch}.abandoned.{stamp}");
            let mut rename = Self::network_command(None);
            rename
                .args(["branch", "-m", branch, &aside])
                .current_dir(path);
            Self::finish(&mut rename).await?;
            tracing::warn!(
                branch,
                %aside,
                "Set aside a branch no worktree held so the run could take the name"
            );
        }
        let mut command = Self::network_command(None);
        command.args(["checkout", "-b", branch]);
        if exists.status.success() {
            command.arg(&remote);
        }
        command.arg("--").current_dir(path);
        Self::finish(&mut command).await
    }

    /// Resolve a commit without reading a caller-controlled symbolic baseline later.
    pub async fn revision(&self, path: &Path, reference: &str) -> GitResult<String> {
        let output = Self::output(
            Self::network_command(None)
                .args([
                    "rev-parse",
                    "--verify",
                    "--end-of-options",
                    &format!("{reference}^{{commit}}"),
                ])
                .current_dir(path)
                .stdout(Stdio::piped()),
        )
        .await?;
        if !output.status.success() {
            return Err(GitError::CommandFailed("Checkout commit is missing".into()));
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    pub async fn is_ancestor(&self, path: &Path, before: &str, after: &str) -> GitResult<bool> {
        let output = Self::output(
            Self::network_command(None)
                .args(["merge-base", "--is-ancestor", before, after])
                .current_dir(path),
        )
        .await?;
        match output.status.code() {
            Some(0) => Ok(true),
            Some(1) => Ok(false),
            _ => Err(GitError::CommandFailed(
                "Cannot verify checkout history".into(),
            )),
        }
    }

    /// Include changes already committed by task tools in the PR description.
    pub async fn changed_files(&self, path: &Path, before: &str) -> GitResult<Vec<String>> {
        let output = Self::output(
            Self::network_command(None)
                .args(["diff", "--name-only", "-z", before, "HEAD", "--"])
                .current_dir(path)
                .stdout(Stdio::piped()),
        )
        .await?;
        if !output.status.success() {
            return Err(GitError::CommandFailed(
                "Cannot inspect task changes".into(),
            ));
        }
        Ok(output
            .stdout
            .split(|byte| *byte == 0)
            .filter(|name| !name.is_empty())
            .map(|name| String::from_utf8_lossy(name).into_owned())
            .collect())
    }

    /// Name a branch after the task it carries: `{prefix}/{short id}-{slug}`.
    pub fn generate_branch_name(&self, task_id: Uuid, title: &str) -> String {
        let short_id = &task_id.to_string()[..8];

        let slug: String = title
            .chars()
            .take(50)
            .map(|character| match character.is_ascii_alphanumeric() {
                true => character.to_ascii_lowercase(),
                false => '-',
            })
            .collect();

        let slug = slug
            .split('-')
            .filter(|part| !part.is_empty())
            .collect::<Vec<&str>>()
            .join("-");

        let branch = format!("{}/{short_id}-{slug}", self.branch_prefix);

        match branch.len() > self.max_branch_length {
            true => branch[..self.max_branch_length].to_string(),
            false => branch,
        }
    }

    /// Whether git considers `path` to be inside a working tree.
    pub async fn is_git_repo(&self, path: &Path) -> GitResult<bool> {
        let output = Self::output(
            Self::network_command(None)
                .arg("rev-parse")
                .arg("--is-inside-work-tree")
                .current_dir(path)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped()),
        )
        .await?;

        Ok(output.status.success())
    }

    /// The branch the checkout is on, or `HEAD` when it is detached.
    pub async fn current_branch(&self, path: &Path) -> GitResult<String> {
        let output = Self::output(
            Self::network_command(None)
                .args(["rev-parse", "--abbrev-ref", "HEAD"])
                .current_dir(path)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped()),
        )
        .await?;

        if !output.status.success() {
            return Err(GitError::CommandFailed(format!(
                "git rev-parse failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }

        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    /// Bring a base clone up to date with the repository at `url`, credentials
    /// in the child environment only. The URL is the one the caller knows, given
    /// on the command line, so nothing a run wrote into the clone's
    /// configuration — a rewritten `remote.origin.url`, an `insteadOf` rule —
    /// decides where the fetch goes; every `refs/remotes/origin/*` ref is forced
    /// to what the remote holds and the ones it no longer has are pruned. A
    /// refusal is retried once, since git's ref locks refuse the loser of a race
    /// with a run's own git commands.
    pub async fn fetch(&self, path: &Path, url: &str, token: Option<&str>) -> GitResult<()> {
        let source = Self::repository_url(url)?;
        self.fetch_source(path, &source, token, false).await
    }

    async fn fetch_source(
        &self,
        path: &Path,
        source: &str,
        token: Option<&str>,
        local: bool,
    ) -> GitResult<()> {
        let mut attempts = 0;
        loop {
            let mut command = Self::network_command(token);
            if local {
                #[cfg(test)]
                command.env("GIT_ALLOW_PROTOCOL", "file");
                #[cfg(not(test))]
                return Err(GitError::CommandFailed(
                    "Local repositories are disabled".to_string(),
                ));
            }
            command
                .args([
                    "fetch",
                    "--prune",
                    "--",
                    source,
                    "+refs/heads/*:refs/remotes/origin/*",
                ])
                .current_dir(path);
            match Self::finish(&mut command).await {
                Ok(()) => return Ok(()),
                Err(error) if attempts == 0 => {
                    attempts += 1;
                    tracing::debug!(%error, "Retrying a fetch another git command may have locked");
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
                Err(error) => return Err(error),
            }
        }
    }

    /// The remote's default branch and the commit at its tip, asked of the
    /// remote itself. `refs/remotes/origin/HEAD` is not consulted: a run's git
    /// commands share it with every other run of the repository, and a fetch
    /// never restores it.
    pub async fn remote_head(
        &self,
        path: &Path,
        url: &str,
        token: Option<&str>,
    ) -> GitResult<RemoteHead> {
        let source = Self::repository_url(url)?;
        self.remote_head_source(path, &source, token, false).await
    }

    async fn remote_head_source(
        &self,
        path: &Path,
        source: &str,
        token: Option<&str>,
        local: bool,
    ) -> GitResult<RemoteHead> {
        let mut command = Self::network_command(token);
        if local {
            #[cfg(test)]
            command.env("GIT_ALLOW_PROTOCOL", "file");
            #[cfg(not(test))]
            return Err(GitError::CommandFailed(
                "Local repositories are disabled".to_string(),
            ));
        }
        command
            .args(["ls-remote", "--symref", "--", source, "HEAD"])
            .current_dir(path)
            .stdout(Stdio::piped());
        let output = Self::output(&mut command).await?;
        if !output.status.success() {
            return Err(GitError::CommandFailed(
                "Git operation failed; verify repository access".to_string(),
            ));
        }
        let listing = String::from_utf8_lossy(&output.stdout);
        let (mut branch, mut commit) = (None, None);
        for line in listing.lines() {
            let Some((left, right)) = line.split_once('\t') else {
                continue;
            };
            if right != "HEAD" {
                continue;
            }
            if let Some(reference) = left.strip_prefix("ref: ") {
                branch = reference.strip_prefix("refs/heads/").map(str::to_string);
            } else if !left.is_empty()
                && left.chars().all(|character| character.is_ascii_hexdigit())
            {
                commit = Some(left.to_string());
            }
        }
        match (branch, commit) {
            (Some(branch), Some(commit)) => Ok(RemoteHead { branch, commit }),
            _ => Err(GitError::CommandFailed(
                "The repository reports no default branch".to_string(),
            )),
        }
    }

    /// Rewrite a base clone's configuration from what the caller knows about it.
    /// The clone's `.git/config` is read by every git command that runs in
    /// it or in a worktree of it, and a run's git commands can write to it:
    /// a `core.fsmonitor` or a filter driver would run as this process on the
    /// next fetch or checkout, an `insteadOf` rule would redirect it. So the
    /// file is replaced before the base is used, carrying over only the
    /// settings git chose for the file system, and the remote's URL is
    /// written through `git config`, which escapes it.
    pub async fn reset_config(&self, path: &Path, url: &str) -> GitResult<()> {
        const CARRIED: [&str; 4] = ["filemode", "ignorecase", "precomposeunicode", "symlinks"];
        if url.chars().any(char::is_control) {
            return Err(GitError::CommandFailed("Invalid repository URL".into()));
        }
        let file = path.join(".git").join("config");
        let listed = Self::output(
            Self::network_command(None)
                .args(["config", "--file"])
                .arg(&file)
                .arg("--list")
                .stdout(Stdio::piped()),
        )
        .await?;
        let mut carried = String::new();
        if listed.status.success() {
            for line in String::from_utf8_lossy(&listed.stdout).lines() {
                if let Some((key, value)) = line.split_once('=')
                    && let Some(name) = key.strip_prefix("core.")
                    && CARRIED.contains(&name)
                    && matches!(value, "true" | "false")
                {
                    carried.push_str(&format!("\t{name} = {value}\n"));
                }
            }
        }
        tokio::fs::write(
            &file,
            format!(
                "[core]\n\trepositoryformatversion = 0\n\tbare = false\n\tlogallrefupdates = true\n{carried}"
            ),
        )
        .await?;
        for (key, value) in [
            ("remote.origin.url", url),
            ("remote.origin.fetch", "+refs/heads/*:refs/remotes/origin/*"),
        ] {
            Self::finish(
                Self::network_command(None)
                    .args(["config", "--file"])
                    .arg(&file)
                    .args([key, value]),
            )
            .await?;
        }
        Ok(())
    }

    /// Whether the repository holds `commit`, given as a hex object name.
    pub async fn has_commit(&self, path: &Path, commit: &str) -> GitResult<bool> {
        if commit.is_empty()
            || !commit
                .chars()
                .all(|character| character.is_ascii_hexdigit())
        {
            return Err(GitError::CommandFailed("Invalid commit".into()));
        }
        let output = Self::output(
            Self::network_command(None)
                .args(["cat-file", "-e", &format!("{commit}^{{commit}}")])
                .current_dir(path),
        )
        .await?;
        Ok(output.status.success())
    }

    /// Point `refs/remotes/origin/HEAD` at the remote's default branch, for
    /// whoever reads the ref; a run is started from the commit
    /// [`Self::remote_head`] reported, never from this ref.
    pub async fn set_remote_head(&self, path: &Path, branch: &str) -> GitResult<()> {
        let reference = format!("refs/remotes/origin/{branch}");
        let valid =
            Self::output(Self::network_command(None).args(["check-ref-format", &reference]))
                .await?;
        if !valid.status.success() || branch.starts_with('-') {
            return Err(GitError::CommandFailed("Invalid default branch".into()));
        }
        Self::finish(
            Self::network_command(None)
                .args(["symbolic-ref", "refs/remotes/origin/HEAD", &reference])
                .current_dir(path),
        )
        .await
    }

    /// Whether the working tree or the index holds anything uncommitted.
    pub async fn has_changes(&self, path: &Path) -> GitResult<bool> {
        let output = Self::output(
            Self::network_command(None)
                .args(["status", "--porcelain"])
                .current_dir(path)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped()),
        )
        .await?;

        if !output.status.success() {
            return Err(GitError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }

        Ok(!output.stdout.is_empty())
    }

    /// Summarise the uncommitted changes, with the diff itself truncated
    /// once it runs past 50 kB.
    pub async fn diff_summary(&self, path: &Path) -> GitResult<DiffSummary> {
        let status_output = Self::output(
            Self::network_command(None)
                .args(["status", "--porcelain"])
                .current_dir(path)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped()),
        )
        .await?;

        if !status_output.status.success() {
            return Err(GitError::CommandFailed(
                String::from_utf8_lossy(&status_output.stderr).to_string(),
            ));
        }

        let status_text = String::from_utf8_lossy(&status_output.stdout);
        let files_changed: Vec<String> = status_text
            .lines()
            .filter(|line| line.len() > 3)
            .map(|line| line[3..].to_string())
            .collect();

        let diff_stat_output = Self::output(
            Self::network_command(None)
                .args(["diff", "--shortstat", "HEAD"])
                .current_dir(path)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped()),
        )
        .await?;

        let mut insertions = 0;
        let mut deletions = 0;

        if diff_stat_output.status.success() {
            let stat_text = String::from_utf8_lossy(&diff_stat_output.stdout);
            for part in stat_text.split(',') {
                let part = part.trim();
                if part.contains("insertion") {
                    if let Some(count) = part.split_whitespace().next() {
                        insertions = count.parse().unwrap_or(0);
                    }
                } else if part.contains("deletion")
                    && let Some(count) = part.split_whitespace().next()
                {
                    deletions = count.parse().unwrap_or(0);
                }
            }
        }

        let diff_output = Self::output(
            Self::network_command(None)
                .args(["diff", "HEAD"])
                .current_dir(path)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped()),
        )
        .await?;

        let diff_text = String::from_utf8_lossy(&diff_output.stdout);
        // len() counts bytes, so cutting at a fixed offset panics whenever the
        // boundary lands inside a multi-byte character -- an emoji or any
        // accented character in a diff over the cap is enough.
        let diff_text = match diff_text.len() > MAXIMUM_DIFF_BYTES {
            true => {
                let mut end = MAXIMUM_DIFF_BYTES;
                while end > 0 && !diff_text.is_char_boundary(end) {
                    end -= 1;
                }
                format!("{}...[truncated]", &diff_text[..end])
            }
            false => diff_text.to_string(),
        };

        Ok(DiffSummary {
            files_changed,
            insertions,
            deletions,
            diff_text,
        })
    }

    /// Create and check out a new branch, refusing a name already taken.
    pub async fn create_branch(&self, path: &Path, branch_name: &str) -> GitResult<()> {
        let check_output = Self::output(
            Self::network_command(None)
                .args([
                    "show-ref",
                    "--verify",
                    "--quiet",
                    &format!("refs/heads/{branch_name}"),
                ])
                .current_dir(path)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped()),
        )
        .await?;

        if check_output.status.success() {
            return Err(GitError::BranchExists(branch_name.to_string()));
        }

        let output = Self::output(
            Self::network_command(None)
                .args(["checkout", "-b", branch_name])
                .current_dir(path)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped()),
        )
        .await?;

        if !output.status.success() {
            return Err(GitError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }

        Ok(())
    }

    /// Stage every change in the working tree.
    pub async fn stage_all(&self, path: &Path) -> GitResult<()> {
        let output = Self::output(
            Self::network_command(None)
                .args(["add", "-A"])
                .current_dir(path)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped()),
        )
        .await?;

        if !output.status.success() {
            return Err(GitError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }

        Ok(())
    }

    /// Commit what is staged, and say which commit it became.
    pub async fn commit(&self, path: &Path, message: &str) -> GitResult<String> {
        let output = Self::output(
            Self::network_command(None)
                .args([
                    "-c",
                    &format!("user.name={}", self.author_name),
                    "-c",
                    &format!("user.email={}", self.author_email),
                    "commit",
                    "-m",
                    message,
                ])
                .current_dir(path)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped()),
        )
        .await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if stderr.contains("nothing to commit") {
                return Err(GitError::NoChanges);
            }
            return Err(GitError::CommandFailed(stderr.to_string()));
        }

        let sha_output = Self::output(
            Self::network_command(None)
                .args(["rev-parse", "HEAD"])
                .current_dir(path)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped()),
        )
        .await?;

        Ok(String::from_utf8_lossy(&sha_output.stdout)
            .trim()
            .to_string())
    }

    /// Push a branch to a named remote.
    pub async fn push(&self, path: &Path, branch_name: &str, remote: &str) -> GitResult<()> {
        let output = Self::output(
            Self::network_command(None)
                .args(["push", "-u", remote, branch_name])
                .current_dir(path)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped()),
        )
        .await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if stderr.contains("Authentication failed")
                || stderr.contains("could not read Username")
            {
                return Err(GitError::AuthenticationFailed);
            }
            return Err(GitError::CommandFailed(stderr.to_string()));
        }

        Ok(())
    }

    /// Push the checkout's HEAD to `branch_name` on the remote, with access
    /// token authentication, and say which commit was pushed. The commit is
    /// resolved before the push and the push names it rather than `HEAD`, so
    /// what the caller is told was pushed is what the remote received even if
    /// something moves HEAD meanwhile.
    pub async fn push_with_token(
        &self,
        path: &Path,
        branch_name: &str,
        remote_url: &str,
        token: &str,
    ) -> GitResult<String> {
        let remote = Self::repository_url(remote_url)?;
        self.push_source(path, branch_name, &remote, token, false)
            .await
    }

    async fn push_source(
        &self,
        path: &Path,
        branch_name: &str,
        remote: &str,
        token: &str,
        local: bool,
    ) -> GitResult<String> {
        let commit = self.revision(path, "HEAD").await?;
        let mut command = Self::network_command(Some(token));
        if local {
            #[cfg(test)]
            command.env("GIT_ALLOW_PROTOCOL", "file");
            #[cfg(not(test))]
            return Err(GitError::CommandFailed(
                "Local repositories are disabled".into(),
            ));
        }
        command
            .args([
                "push",
                "--porcelain",
                "--",
                remote,
                &format!("{commit}:refs/heads/{branch_name}"),
            ])
            .current_dir(path)
            .stdout(Stdio::piped());
        let output = Self::output(&mut command).await?;
        if output.status.success() {
            // What was pushed is what origin has, said in the repository's
            // own terms for whoever reads its refs; the service itself keeps
            // the commit it was told, since a ref in a shared clone proves
            // nothing to it.
            let mut tracking = Self::network_command(None);
            tracking
                .args([
                    "update-ref",
                    &format!("refs/remotes/origin/{branch_name}"),
                    &commit,
                ])
                .current_dir(path);
            if let Err(error) = Self::finish(&mut tracking).await {
                tracing::warn!(%error, "Pushed, but could not record the remote-tracking ref");
            }
            return Ok(commit);
        }
        // Classify only Git's machine-readable status; never expose remote output
        // or URLs that could contain credentials or untrusted server messages.
        let rejected = output
            .stdout
            .split(|byte| *byte == b'\n')
            .any(|line| line.starts_with(b"!\t"));
        Err(GitError::CommandFailed(match rejected {
            true => {
                "Git push rejected; remote history or policy changed. Retry from a fresh checkout"
                    .into()
            }
            false => "Git push failed; verify repository access".into(),
        }))
    }

    /// The URL configured for a named remote.
    pub async fn get_remote_url(&self, path: &Path, remote: &str) -> GitResult<String> {
        let output = Self::output(
            Self::network_command(None)
                .args(["remote", "get-url", remote])
                .current_dir(path)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped()),
        )
        .await?;

        if !output.status.success() {
            return Err(GitError::NoRemote);
        }

        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    /// Check out an existing branch.
    pub async fn checkout(&self, path: &Path, branch_name: &str) -> GitResult<()> {
        let output = Self::output(
            Self::network_command(None)
                .args(["checkout", branch_name])
                .current_dir(path)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped()),
        )
        .await?;

        if !output.status.success() {
            return Err(GitError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).to_string(),
            ));
        }

        Ok(())
    }

    /// A git invocation against a clone the caller owns outright, run with the
    /// caller's own environment so an address only that environment can reach —
    /// a local path, an SSH remote, a credential helper — still works.
    fn managed_command(path: Option<&Path>) -> Command {
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
    fn make_absolute(path: &Path) -> GitResult<PathBuf> {
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

    /// Add a worktree of a managed clone at `worktree_path`, in detached HEAD
    /// state at `checkout_ref`.
    ///
    /// A directory already standing there is removed first, so a worktree a
    /// crashed run left behind does not refuse the next one.
    pub async fn create_worktree(
        &self,
        path: &Path,
        worktree_path: &Path,
        checkout_ref: &str,
    ) -> GitResult<()> {
        validate_reference(checkout_ref, "checkout ref")?;

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
            checkout_ref,
            "Creating worktree"
        );

        let output = Self::managed_command(Some(path))
            .args(["worktree", "add", "--detach"])
            .arg(worktree_path)
            .arg(checkout_ref)
            .output()
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
        branch: &str,
        start_point: &str,
    ) -> GitResult<()> {
        validate_reference(branch, "branch")?;
        validate_reference(start_point, "start point")?;

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
            branch,
            start_point,
            "Creating worktree on branch"
        );

        let output = Self::managed_command(Some(path))
            .args(["worktree", "add", "-B"])
            .arg(branch)
            .arg(worktree_path)
            .arg(start_point)
            .output()
            .await?;

        if !output.status.success() {
            return Err(GitError::CommandFailed(format!(
                "git worktree add -B {branch} failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }

        tracing::info!(worktree = ?worktree_path, branch, "Worktree created on branch");
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

        let output = Self::managed_command(Some(path))
            .args(["worktree", "remove", "--force"])
            .arg(worktree_path)
            .output()
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
                return Err(GitError::UnsafeWorktree(
                    worktree_path.display().to_string(),
                ));
            }
            tokio::fs::remove_dir_all(worktree_path).await?;
        }

        let _ = Self::managed_command(Some(path))
            .args(["worktree", "prune"])
            .output()
            .await;

        tracing::debug!(worktree = ?worktree_path, "Worktree removed");
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
fn validate_reference(name: &str, label: &str) -> GitResult<()> {
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

/// Authenticate a git network command without putting the token in the URL.
///
/// A credential in the remote URL reaches `.git/config`, the process table and
/// any error text that echoes the remote. The header is scoped to github.com so
/// a redirect elsewhere cannot carry it.
pub(crate) fn authenticate(command: &mut Command, token: &str) {
    let authorization =
        base64::engine::general_purpose::STANDARD.encode(format!("x-access-token:{token}"));
    command
        .env("GIT_CONFIG_COUNT", "1")
        .env("GIT_CONFIG_KEY_0", "http.https://github.com/.extraHeader")
        .env(
            "GIT_CONFIG_VALUE_0",
            format!("Authorization: Basic {authorization}"),
        );
}

#[cfg(test)]
mod branch_name_tests {
    use super::*;

    fn task() -> Uuid {
        Uuid::parse_str("12345678-1234-1234-1234-123456789abc").unwrap()
    }

    #[test]
    fn a_branch_is_named_after_the_task_and_its_title() {
        let branch = GitService::new().generate_branch_name(task(), "Fix the login bug");

        assert!(branch.starts_with("task/12345678-"), "{branch}");
        assert!(branch.contains("fix-the-login-bug"), "{branch}");
        assert_eq!(branch, branch.to_lowercase());
    }

    #[test]
    fn anything_a_branch_name_may_not_carry_becomes_a_single_hyphen() {
        let branch = GitService::new().generate_branch_name(task(), "Add user@email validation!!!");

        assert!(!branch.contains('@'), "{branch}");
        assert!(!branch.contains('!'), "{branch}");
        assert!(!branch.contains("--"), "{branch}");
    }

    #[test]
    fn a_title_longer_than_the_name_allows_is_cut_to_it() {
        let branch = GitService::new().generate_branch_name(task(), &"A".repeat(200));

        assert!(branch.len() <= MAXIMUM_BRANCH_LENGTH, "{branch}");
    }

    #[test]
    fn a_title_in_another_script_still_names_a_branch_git_accepts() {
        let branch = GitService::new().generate_branch_name(task(), "修复登录问题");

        assert!(branch.is_ascii(), "{branch}");
        assert!(branch.starts_with("task/12345678-"), "{branch}");
    }

    #[test]
    fn a_title_with_nothing_in_it_still_names_the_task() {
        for title in ["", "   "] {
            let branch = GitService::new().generate_branch_name(task(), title);

            assert!(branch.starts_with("task/12345678-"), "{branch}");
            assert!(!branch.contains(' '), "{branch}");
        }
    }

    #[test]
    fn the_namespace_the_author_and_the_length_are_the_callers_to_choose() {
        let service = GitService::new()
            .with_branch_prefix("agent")
            .with_author("Ada", "ada@example.test")
            .with_max_branch_length(20);

        let branch = service.generate_branch_name(task(), "a title far longer than twenty");

        assert!(branch.starts_with("agent/12345678-"), "{branch}");
        assert_eq!(branch.len(), 20, "{branch}");
        assert_eq!(service.author_name, "Ada");
        assert_eq!(service.author_email, "ada@example.test");
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
mod checkout_tests {
    use super::*;

    #[test]
    fn rejects_unsafe_repository_inputs() {
        for source in [
            "--upload-pack=evil",
            "/tmp/repository",
            "file:///tmp/repository",
            "ext::command",
            "git@github.com:owner/repository",
            "https://token@github.com/owner/repository",
            "https://github.com/owner/repository?token=secret",
            "https://elsewhere.test/owner/repository",
            "https://github.com/owner/repository/extra",
        ] {
            assert!(GitService::repository_url(source).is_err(), "{source}");
        }
        assert_eq!(
            GitService::repository_url("https://github.com/owner/repository").unwrap(),
            "https://github.com/owner/repository.git"
        );
    }

    #[test]
    fn authentication_is_not_in_arguments_or_repository_config() {
        let command = GitService::network_command(Some("sensitive-token"));
        let arguments = format!("{:?}", command.as_std().get_args().collect::<Vec<_>>());
        assert!(!arguments.contains("sensitive-token"));
        assert!(arguments.contains("credential.helper="));
        assert!(arguments.contains("http.followRedirects=false"));
    }

    #[tokio::test]
    async fn failed_clone_prevents_execution() {
        let fixture = tempfile::tempdir().unwrap();
        let mut executed = false;
        let result = async {
            GitService::new()
                .clone_source(
                    fixture.path().join("missing").to_str().unwrap(),
                    &fixture.path().join("checkout"),
                    Some("sensitive-token"),
                    true,
                )
                .await?;
            executed = true;
            Ok::<(), GitError>(())
        }
        .await;
        assert!(result.is_err());
        assert!(!executed);
        assert!(!result.unwrap_err().to_string().contains("sensitive-token"));
    }

    #[tokio::test]
    async fn clone_contains_committed_sentinel() {
        let fixture = tempfile::tempdir().unwrap();
        crate::worktree::fixtures::git(fixture.path(), &["init", "-q"]);
        std::fs::write(fixture.path().join("sentinel"), "committed").unwrap();
        crate::worktree::fixtures::git(fixture.path(), &["add", "sentinel"]);
        crate::worktree::fixtures::git(fixture.path(), &["commit", "-q", "-m", "fixture"]);
        let destination = tempfile::tempdir().unwrap();
        let path = destination.path().join("checkout");
        GitService::new()
            .clone_source(fixture.path().to_str().unwrap(), &path, None, true)
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(path.join("sentinel")).unwrap(),
            "committed"
        );
        assert!(path.join(".git").is_dir());
        let second = destination.path().join("second");
        GitService::new()
            .clone_source(
                fixture.path().to_str().unwrap(),
                &second,
                Some("sensitive-token"),
                true,
            )
            .await
            .unwrap();
        std::fs::write(path.join("sentinel"), "modified").unwrap();
        assert_eq!(
            std::fs::read_to_string(second.join("sentinel")).unwrap(),
            "committed"
        );
        let config = std::fs::read_to_string(second.join(".git/config")).unwrap();
        assert!(!config.contains("sensitive-token"));
        assert!(!config.contains("Authorization"));
        let service = GitService::new().with_author("Fixture", "fixture@example.test");
        service.stage_all(&path).await.unwrap();
        assert!(
            !service
                .commit(&path, "task change")
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            crate::worktree::fixtures::git(&path, &["show", "-s", "--format=%an <%ae>"]),
            "Fixture <fixture@example.test>"
        );
    }
}

#[cfg(all(test, unix))]
mod process_tests {
    use super::*;
    use nix::sys::signal::{Signal, kill};
    use nix::unistd::Pid;
    use std::os::unix::fs::PermissionsExt;

    /// These fixtures share the machine with every other test binary, and under
    /// coverage instrumentation all of it is slower. The budgets only bound how
    /// long a genuine regression takes to surface, so they are generous.
    const SPAWN_BUDGET: Duration = Duration::from_secs(30);
    const TEARDOWN_BUDGET: Duration = Duration::from_secs(10);

    async fn marker(directory: &Path, name: &str) -> u32 {
        tokio::time::timeout(SPAWN_BUDGET, async {
            loop {
                if let Ok(value) = tokio::fs::read_to_string(directory.join(name)).await
                    && let Ok(pid) = value.trim().parse()
                {
                    return pid;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("fixture process did not start")
    }

    fn alive(pid: u32) -> bool {
        kill(Pid::from_raw(pid as i32), None).is_ok()
    }

    async fn cancellation(timeout: bool) {
        let fixture = tempfile::tempdir().unwrap();
        std::fs::write(
            fixture.path().join("git"),
            "#!/bin/sh\necho $$ > \"$FIXTURE/parent\"\n/bin/sh \"$FIXTURE/helper\" &\nwait\n",
        )
        .unwrap();
        std::fs::write(fixture.path().join("helper"), "#!/bin/sh\necho $$ > \"$FIXTURE/helper-pid\"\nprintf '%s' \"$GIT_CONFIG_VALUE_0\" > \"$FIXTURE/credential\"\n/bin/sh \"$FIXTURE/grandchild\" &\nwait\n").unwrap();
        std::fs::write(
            fixture.path().join("grandchild"),
            "#!/bin/sh\necho $$ > \"$FIXTURE/grandchild-pid\"\nexec /bin/sleep 60\n",
        )
        .unwrap();
        std::fs::set_permissions(
            fixture.path().join("git"),
            std::fs::Permissions::from_mode(0o700),
        )
        .unwrap();
        let mut unrelated = Command::new("/bin/sleep")
            .arg("60")
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let mut command = GitService::network_command(Some("fixture-credential"));
        command
            .env("PATH", fixture.path())
            .env("FIXTURE", fixture.path());
        let operation = tokio::spawn(async move { GitService::finish(&mut command).await });
        let parent = marker(fixture.path(), "parent").await;
        let helper = marker(fixture.path(), "helper-pid").await;
        let grandchild = marker(fixture.path(), "grandchild-pid").await;
        let credential = tokio::fs::read_to_string(fixture.path().join("credential"))
            .await
            .unwrap();
        assert!(credential.starts_with("Authorization: Basic "));
        if timeout {
            tokio::time::pause();
            tokio::time::advance(Duration::from_secs(301)).await;
            let result = operation.await.unwrap();
            tokio::time::resume();
            assert!(
                matches!(result, Err(GitError::CommandFailed(ref error)) if error.contains("timed out"))
            );
        } else {
            operation.abort();
            assert!(operation.await.unwrap_err().is_cancelled());
        }
        let stopped = tokio::time::timeout(TEARDOWN_BUDGET, async {
            while [parent, helper, grandchild].iter().any(|pid| alive(*pid)) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .is_ok();
        let survivor = unrelated.try_wait().unwrap().is_none();
        // Clean up only this fixture's known processes, including on the seen-red path.
        for pid in [grandchild, helper, parent] {
            let _ = kill(Pid::from_raw(pid as i32), Signal::SIGKILL);
        }
        unrelated.kill().await.unwrap();
        tokio::time::timeout(TEARDOWN_BUDGET, async {
            while [parent, helper, grandchild].iter().any(|pid| alive(*pid)) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("fixture cleanup left a Git helper running");
        assert!(
            survivor,
            "cancellation killed an unrelated process outside the Git group"
        );
        assert!(
            stopped,
            "credential-bearing helper or grandchild survived Git cancellation"
        );
    }

    #[tokio::test]
    async fn dropped_git_future_kills_helpers_and_preserves_unrelated_processes() {
        cancellation(false).await;
    }

    #[tokio::test]
    async fn timed_out_git_kills_helpers_and_preserves_unrelated_processes() {
        cancellation(true).await;
    }
}

#[cfg(test)]
mod publication_tests {
    use super::*;
    use crate::worktree::fixtures::git;

    async fn forged_ancestry(legacy: bool) {
        let fixture = tempfile::tempdir().unwrap();
        git(fixture.path(), &["init", "--initial-branch=main"]);
        let service = GitService::new();
        std::fs::write(fixture.path().join("file"), "baseline").unwrap();
        service.stage_all(fixture.path()).await.unwrap();
        let baseline = service.commit(fixture.path(), "baseline").await.unwrap();
        git(fixture.path(), &["checkout", "--orphan", "task"]);
        std::fs::write(fixture.path().join("file"), "unrelated").unwrap();
        service.stage_all(fixture.path()).await.unwrap();
        let orphan = service.commit(fixture.path(), "orphan").await.unwrap();
        if legacy {
            std::fs::write(
                fixture.path().join(".git/info/grafts"),
                format!("{orphan} {baseline}\n"),
            )
            .unwrap();
        } else {
            git(fixture.path(), &["replace", "--graft", "HEAD", &baseline]);
        }
        assert!(
            !service
                .is_ancestor(fixture.path(), &baseline, "HEAD")
                .await
                .unwrap(),
            "task metadata forged baseline ancestry"
        );
        assert_eq!(
            service.revision(fixture.path(), "HEAD").await.unwrap(),
            orphan
        );
        assert_eq!(
            service
                .changed_files(fixture.path(), &baseline)
                .await
                .unwrap(),
            vec!["file"]
        );
    }

    #[tokio::test]
    async fn replacement_refs_do_not_supply_trusted_ancestry() {
        forged_ancestry(false).await;
    }

    #[tokio::test]
    async fn legacy_grafts_do_not_supply_trusted_ancestry() {
        forged_ancestry(true).await;
    }

    #[tokio::test]
    async fn replacement_objects_cannot_hide_staged_changes() {
        let fixture = tempfile::tempdir().unwrap();
        git(fixture.path(), &["init", "--initial-branch=main"]);
        let service = GitService::new();
        std::fs::write(fixture.path().join("file"), "baseline").unwrap();
        service.stage_all(fixture.path()).await.unwrap();
        let baseline = service.commit(fixture.path(), "baseline").await.unwrap();
        git(fixture.path(), &["checkout", "-b", "replacement"]);
        std::fs::write(fixture.path().join("file"), "changed").unwrap();
        service.stage_all(fixture.path()).await.unwrap();
        let replacement = service
            .commit(fixture.path(), "replacement tree")
            .await
            .unwrap();
        git(fixture.path(), &["checkout", "main"]);
        git(fixture.path(), &["replace", &baseline, &replacement]);
        std::fs::write(fixture.path().join("file"), "changed").unwrap();
        git(fixture.path(), &["--no-replace-objects", "add", "-A"]);
        assert!(
            service.has_changes(fixture.path()).await.unwrap(),
            "replacement tree hid staged task changes"
        );
        let committed = service
            .commit(fixture.path(), "real task changes")
            .await
            .unwrap();
        assert_ne!(committed, baseline);
        assert_eq!(
            service
                .changed_files(fixture.path(), &baseline)
                .await
                .unwrap(),
            vec!["file"]
        );
    }

    #[tokio::test]
    async fn branch_resumption_preserves_commits_and_normal_push_rejects_divergence() {
        let fixture = tempfile::tempdir().unwrap();
        let source = fixture.path().join("source");
        std::fs::create_dir(&source).unwrap();
        git(&source, &["init", "--initial-branch=main"]);
        std::fs::write(source.join("initial"), "base").unwrap();
        let service = GitService::new();
        service.stage_all(&source).await.unwrap();
        service.commit(&source, "initial").await.unwrap();
        let remote = fixture.path().join("remote.git");
        git(&source, &["clone", "--bare", ".", remote.to_str().unwrap()]);
        let first = fixture.path().join("first");
        service
            .clone_source(remote.to_str().unwrap(), &first, None, true)
            .await
            .unwrap();
        service
            .prepare_branch(&first, "task/one", false)
            .await
            .unwrap();
        std::fs::write(first.join("first"), "first attempt").unwrap();
        service.stage_all(&first).await.unwrap();
        let first_commit = service.commit(&first, "first attempt").await.unwrap();
        service
            .push_source(
                &first,
                "task/one",
                remote.to_str().unwrap(),
                "sensitive-token",
                true,
            )
            .await
            .unwrap();
        let second = fixture.path().join("second");
        service
            .clone_source(remote.to_str().unwrap(), &second, None, true)
            .await
            .unwrap();
        service
            .prepare_branch(&second, "task/one", true)
            .await
            .unwrap();
        assert_eq!(
            service.revision(&second, "HEAD").await.unwrap(),
            first_commit
        );
        assert_eq!(
            std::fs::read_to_string(second.join("first")).unwrap(),
            "first attempt"
        );
        std::fs::write(second.join("second"), "second attempt").unwrap();
        service.stage_all(&second).await.unwrap();
        let second_commit = service.commit(&second, "second attempt").await.unwrap();
        let pushed = service
            .push_source(
                &second,
                "task/one",
                remote.to_str().unwrap(),
                "sensitive-token",
                true,
            )
            .await
            .unwrap();
        assert_eq!(pushed, second_commit, "the push says which commit it sent");
        assert_eq!(git(&remote, &["rev-parse", "task/one"]), second_commit);
        std::fs::write(first.join("concurrent"), "stale checkout").unwrap();
        service.stage_all(&first).await.unwrap();
        service.commit(&first, "concurrent attempt").await.unwrap();
        let failure = service
            .push_source(
                &first,
                "task/one",
                remote.to_str().unwrap(),
                "sensitive-token",
                true,
            )
            .await
            .unwrap_err()
            .to_string();
        assert!(failure.contains("Git push rejected"), "{failure}");
        assert!(
            !failure.contains("sensitive-token") && !failure.contains(remote.to_str().unwrap())
        );
        assert_eq!(git(&remote, &["rev-parse", "task/one"]), second_commit);
        assert!(
            !std::fs::read_to_string(first.join(".git/config"))
                .unwrap()
                .contains("sensitive-token")
        );
        assert!(
            service
                .is_ancestor(&second, &first_commit, "HEAD")
                .await
                .unwrap()
        );
    }

    /// A branch an earlier run's kept worktree still holds is refused with
    /// the reason, not with git's "already exists".
    #[tokio::test]
    async fn a_branch_a_kept_worktree_holds_is_refused_with_the_reason() {
        let root = tempfile::tempdir().unwrap();
        let remote = root.path().join("remote");
        std::fs::create_dir(&remote).unwrap();
        crate::worktree::fixtures::remote(&remote);
        let base = root.path().join("base");
        crate::worktree::fixtures::clone(&remote, &base);
        let first = root.path().join("first");
        crate::worktree::add(&base, &first, "origin/HEAD").unwrap();
        let service = GitService::new();
        service
            .prepare_branch(&first, "task/one", false)
            .await
            .unwrap();
        assert_eq!(service.current_branch(&first).await.unwrap(), "task/one");
        assert!(
            service
                .prepare_branch(&first, "task/one", false)
                .await
                .is_ok(),
            "the worktree already on the branch is prepared again without complaint"
        );
        let second = root.path().join("second");
        crate::worktree::add(&base, &second, "origin/HEAD").unwrap();
        let refused = service
            .prepare_branch(&second, "task/one", false)
            .await
            .unwrap_err()
            .to_string();
        assert!(
            refused.contains("earlier run's worktree still holds"),
            "{refused}"
        );
    }

    /// Two remotes that look alike, one of which a run pointed the clone at.
    /// A fetch is bound to the URL it is given, and prunes what that remote no
    /// longer has.
    #[tokio::test]
    async fn a_fetch_goes_to_the_url_it_is_given_and_not_where_the_clone_was_pointed() {
        let root = tempfile::tempdir().unwrap();
        let genuine = root.path().join("genuine");
        std::fs::create_dir(&genuine).unwrap();
        crate::worktree::fixtures::remote(&genuine);
        let rogue = root.path().join("rogue");
        std::fs::create_dir(&rogue).unwrap();
        crate::worktree::fixtures::remote(&rogue);
        std::fs::write(rogue.join("ROGUE"), "planted\n").unwrap();
        git(&rogue, &["add", "ROGUE"]);
        git(&rogue, &["commit", "-q", "-m", "rogue"]);
        let base = root.path().join("base");
        crate::worktree::fixtures::clone(&genuine, &base);
        std::fs::write(genuine.join("NEW"), "moved on\n").unwrap();
        git(&genuine, &["add", "NEW"]);
        git(&genuine, &["commit", "-q", "-m", "moved on"]);
        git(&genuine, &["branch", "gone"]);
        // A run redirected the clone's remote.
        git(
            &base,
            &["config", "remote.origin.url", rogue.to_str().unwrap()],
        );
        let service = GitService::new();
        service
            .fetch_source(&base, genuine.to_str().unwrap(), None, true)
            .await
            .unwrap();
        assert_eq!(
            git(&base, &["rev-parse", "refs/remotes/origin/main"]),
            git(&genuine, &["rev-parse", "main"]),
            "the genuine remote's tip, not the rogue's"
        );
        assert_eq!(
            git(&base, &["rev-parse", "refs/remotes/origin/gone"]),
            git(&genuine, &["rev-parse", "gone"])
        );
        git(&genuine, &["branch", "-D", "gone"]);
        service
            .fetch_source(&base, genuine.to_str().unwrap(), None, true)
            .await
            .unwrap();
        let refs = git(&base, &["for-each-ref", "refs/remotes/origin/"]);
        assert!(!refs.contains("origin/gone"), "{refs}");
    }

    /// The default branch and its tip come from the remote; a redirected
    /// `origin/HEAD` in the clone changes nothing about the answer, and the
    /// ref is set back to the branch the remote named.
    #[tokio::test]
    async fn the_remote_head_is_asked_of_the_remote_and_not_read_from_a_shared_ref() {
        let root = tempfile::tempdir().unwrap();
        let remote = root.path().join("remote");
        std::fs::create_dir(&remote).unwrap();
        crate::worktree::fixtures::remote(&remote);
        let base = root.path().join("base");
        crate::worktree::fixtures::clone(&remote, &base);
        git(
            &base,
            &[
                "symbolic-ref",
                "refs/remotes/origin/HEAD",
                "refs/remotes/origin/planted",
            ],
        );
        let service = GitService::new();
        let head = service
            .remote_head_source(&base, remote.to_str().unwrap(), None, true)
            .await
            .unwrap();
        assert_eq!(head.branch, "main");
        assert_eq!(head.commit, git(&remote, &["rev-parse", "main"]));
        assert!(service.has_commit(&base, &head.commit).await.unwrap());
        assert!(!service.has_commit(&base, &"0".repeat(40)).await.unwrap());
        assert!(
            service.has_commit(&base, "HEAD").await.is_err(),
            "a name is not a commit"
        );
        service.set_remote_head(&base, &head.branch).await.unwrap();
        assert_eq!(
            git(&base, &["symbolic-ref", "refs/remotes/origin/HEAD"]),
            "refs/remotes/origin/main"
        );
        assert!(service.set_remote_head(&base, "-x").await.is_err());
    }

    /// What a run planted in the clone's configuration is gone once it is
    /// reset, what git chose for the file system stays, and the remote is the
    /// one the caller knows.
    #[tokio::test]
    async fn resetting_the_configuration_drops_what_a_run_planted_and_keeps_the_remote() {
        let root = tempfile::tempdir().unwrap();
        let remote = root.path().join("remote");
        std::fs::create_dir(&remote).unwrap();
        crate::worktree::fixtures::remote(&remote);
        let base = root.path().join("base");
        crate::worktree::fixtures::clone(&remote, &base);
        git(&base, &["config", "core.fsmonitor", "/bin/false"]);
        git(
            &base,
            &[
                "config",
                "url.https://evil.example/.insteadOf",
                "https://github.com/",
            ],
        );
        git(
            &base,
            &["config", "remote.origin.url", "https://evil.example/x.git"],
        );
        git(&base, &["config", "filter.planted.smudge", "/bin/false"]);
        let service = GitService::new();
        service
            .reset_config(&base, "https://github.com/fixture/repository.git")
            .await
            .unwrap();
        let listed = git(&base, &["config", "--file", ".git/config", "--list"]);
        assert!(!listed.contains("fsmonitor"), "{listed}");
        assert!(!listed.contains("insteadof"), "{listed}");
        assert!(!listed.contains("filter."), "{listed}");
        assert!(!listed.contains("evil"), "{listed}");
        assert!(
            listed.contains("remote.origin.url=https://github.com/fixture/repository.git"),
            "{listed}"
        );
        assert!(
            listed.contains("remote.origin.fetch=+refs/heads/*:refs/remotes/origin/*"),
            "{listed}"
        );
        assert!(listed.contains("core.filemode="), "{listed}");
        assert_eq!(
            git(&base, &["status", "--porcelain"]),
            "",
            "the clone still works"
        );
        assert!(
            service
                .reset_config(&base, "https://github.com/x/y.git\n[core]")
                .await
                .is_err()
        );
    }

    /// A branch that no worktree holds — left behind by a removal whose
    /// deletion failed, or by a worktree deleted by hand — is taken over by
    /// the next run that asks for it, and what it pointed at is set aside
    /// under a name of its own rather than lost.
    #[tokio::test]
    async fn a_branch_no_worktree_holds_is_set_aside_and_its_name_taken_by_the_next_run() {
        let root = tempfile::tempdir().unwrap();
        let remote = root.path().join("remote");
        std::fs::create_dir(&remote).unwrap();
        crate::worktree::fixtures::remote(&remote);
        let base = root.path().join("base");
        crate::worktree::fixtures::clone(&remote, &base);
        let service = GitService::new();

        let first = root.path().join("first");
        crate::worktree::add(&base, &first, "origin/HEAD").unwrap();
        service
            .prepare_branch(&first, "task/one", false)
            .await
            .unwrap();
        std::fs::write(first.join("work.txt"), "never pushed\n").unwrap();
        git(&first, &["add", "work.txt"]);
        git(&first, &["commit", "-q", "-m", "work"]);
        let orphaned = git(&first, &["rev-parse", "HEAD"]);
        git(
            &base,
            &["worktree", "remove", "--force", first.to_str().unwrap()],
        );
        assert!(
            git(&base, &["branch", "--list", "task/one"]).contains("task/one"),
            "the branch outlived its worktree"
        );
        let second = root.path().join("second");
        crate::worktree::add(&base, &second, "origin/HEAD").unwrap();
        service
            .prepare_branch(&second, "task/one", false)
            .await
            .unwrap();
        assert_eq!(service.current_branch(&second).await.unwrap(), "task/one");
        assert_ne!(
            git(&second, &["rev-parse", "HEAD"]),
            orphaned,
            "the run starts fresh"
        );
        let aside = git(
            &base,
            &[
                "for-each-ref",
                "--format=%(refname:short) %(objectname)",
                "refs/heads/task/one.abandoned.*",
            ],
        );
        assert!(
            aside.contains("task/one.abandoned.") && aside.contains(&orphaned),
            "the orphaned commit is kept under a name of its own: {aside}"
        );

        let third = root.path().join("third");
        crate::worktree::add(&base, &third, "origin/HEAD").unwrap();
        service
            .prepare_branch(&third, "task/other", false)
            .await
            .unwrap();
        std::fs::remove_dir_all(&third).unwrap();
        let fourth = root.path().join("fourth");
        crate::worktree::add(&base, &fourth, "origin/HEAD").unwrap();
        service
            .prepare_branch(&fourth, "task/other", false)
            .await
            .unwrap();
        assert_eq!(service.current_branch(&fourth).await.unwrap(), "task/other");
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
