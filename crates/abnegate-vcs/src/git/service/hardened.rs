use super::*;
use crate::git::WorktreeEntry;
use tokio::io::AsyncWriteExt;

impl GitService {
    /// Clone into an empty, caller-owned directory. Credentials live only in the
    /// child environment, never the origin URL, process arguments, or git config.
    pub async fn clone_repository(
        &self,
        source: &RepositoryUrl,
        destination: &Path,
        token: Option<&SecretValue>,
    ) -> GitResult<()> {
        let mut command = Self::connected(source, token);
        command
            .args([
                "clone",
                "--no-hardlinks",
                "--template=",
                "--",
                source.as_str(),
            ])
            .arg(destination);
        Self::finish(&mut command).await
    }

    /// Resume the task branch from a fresh clone without rewriting its history.
    pub async fn prepare_branch(
        &self,
        path: &Path,
        branch: &BranchName,
        required: bool,
    ) -> GitResult<()> {
        Self::verify_config(path).await?;
        let reference = branch.reference();
        let remote = format!("{REMOTE_TRACKING}{branch}");
        let exists = Self::output(
            Self::hardened()
                .args(["show-ref", "--verify", "--quiet", &remote])
                .current_dir(path),
        )
        .await?;
        if !exists.status.success() && (required || exists.status.code() != Some(1)) {
            return Err(GitError::CommandFailed(
                "Task branch is missing from the repository".into(),
            ));
        }
        let checked_out = Self::output(
            Self::hardened()
                .args(["symbolic-ref", "--quiet", "HEAD"])
                .current_dir(path)
                .stdout(Stdio::piped()),
        )
        .await?;
        if String::from_utf8_lossy(&checked_out.stdout).trim() == reference {
            return Ok(());
        }
        let held = Self::output(
            Self::hardened()
                .args(["show-ref", "--verify", "--quiet", &reference])
                .current_dir(path),
        )
        .await?;
        if held.status.success() {
            let mut prune = Self::hardened();
            prune.args(["worktree", "prune"]).current_dir(path);
            let _ = Self::finish(&mut prune).await;
            let listed = Self::output(
                Self::hardened()
                    .args(["worktree", "list", "--porcelain", "-z"])
                    .current_dir(path)
                    .stdout(Stdio::piped()),
            )
            .await?;
            if WorktreeEntry::parse(&listed.stdout)
                .iter()
                .any(|entry| entry.branch.as_deref() == Some(reference.as_str()))
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
            let aside = BranchName::parse(&format!("{branch}{ABANDONED}{stamp}"))?;
            let mut rename = Self::hardened();
            rename
                .args(["branch", "-m", "--", branch.as_str(), aside.as_str()])
                .current_dir(path);
            Self::finish(&mut rename).await?;
            tracing::warn!(
                %branch,
                %aside,
                "Set aside a branch no worktree held so the run could take the name"
            );
        }
        let mut command = Self::hardened();
        command.args(["checkout", "-b", branch.as_str()]);
        if exists.status.success() {
            command.arg(&remote);
        }
        command.arg("--").current_dir(path);
        Self::finish(&mut command).await
    }

    /// Resolve a commit without reading a caller-controlled symbolic baseline later.
    pub async fn revision(&self, path: &Path, reference: &str) -> GitResult<CommitSha> {
        Self::verify_config(path).await?;
        let output = Self::output(
            Self::hardened()
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
        Ok(CommitSha::parse(&String::from_utf8_lossy(&output.stdout))?)
    }

    /// Whether `before` is an ancestor of `after`, by the stored objects alone.
    pub async fn is_ancestor(
        &self,
        path: &Path,
        before: &CommitSha,
        after: &CommitSha,
    ) -> GitResult<bool> {
        Self::verify_config(path).await?;
        let output = Self::output(
            Self::hardened()
                .args([
                    "merge-base",
                    "--is-ancestor",
                    "--end-of-options",
                    before.as_str(),
                    after.as_str(),
                ])
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
    pub async fn changed_files(&self, path: &Path, before: &CommitSha) -> GitResult<Vec<String>> {
        Self::verify_config(path).await?;
        let output = Self::output(
            Self::hardened()
                .args([
                    "diff",
                    "--no-ext-diff",
                    "--no-textconv",
                    "--name-only",
                    "-z",
                    "--end-of-options",
                    before.as_str(),
                    "HEAD",
                    "--",
                ])
                .current_dir(path)
                .stdout(Stdio::piped()),
        )
        .await?;
        if !output.status.success() {
            return Err(GitError::CommandFailed(
                "Cannot inspect task changes".into(),
            ));
        }
        Ok(split_nul(&output.stdout))
    }

    /// Whether git considers `path` to be inside a working tree.
    pub async fn is_git_repository(&self, path: &Path) -> GitResult<bool> {
        let output = Self::output(
            Self::hardened()
                .args(["rev-parse", "--is-inside-work-tree"])
                .current_dir(path)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped()),
        )
        .await?;

        Ok(output.status.success())
    }

    /// The branch the checkout is on, or `HEAD` when it is detached.
    pub async fn current_branch(&self, path: &Path) -> GitResult<String> {
        Self::verify_config(path).await?;
        let output = Self::output(
            Self::hardened()
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
    /// on the command line, so a rewritten `remote.origin.url` does not decide
    /// where the fetch goes, and a clone whose configuration could still
    /// redirect it or run something during it -- an `insteadOf` rule, an
    /// `http.*` override, a remote named for the URL, a configured hook -- is
    /// refused rather than fetched; every
    /// `refs/remotes/origin/*` ref is forced to what the remote holds and the
    /// ones it no longer has are pruned. A refusal by git is retried once,
    /// since git's ref locks refuse the loser of a race with a run's own git
    /// commands.
    pub async fn fetch(
        &self,
        path: &Path,
        url: &RepositoryUrl,
        token: Option<&SecretValue>,
    ) -> GitResult<()> {
        let mut attempts = 0;
        loop {
            Self::verify_config(path).await?;
            let mut command = Self::connected(url, token);
            command
                .args(["fetch", "--prune", "--", url.as_str(), FETCH_REFSPEC])
                .current_dir(path);
            match Self::finish(&mut command).await {
                Ok(()) => return Ok(()),
                Err(error) if attempts == 0 => {
                    attempts += 1;
                    tracing::debug!(%error, "Retrying a fetch another git command may have locked");
                    tokio::time::sleep(RETRY_DELAY).await;
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
        url: &RepositoryUrl,
        token: Option<&SecretValue>,
    ) -> GitResult<RemoteHead> {
        Self::verify_config(path).await?;
        let mut command = Self::connected(url, token);
        command
            .args(["ls-remote", "--symref", "--", url.as_str(), "HEAD"])
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
            match left.strip_prefix(SYMBOLIC_REFERENCE) {
                Some(reference) => {
                    branch = reference
                        .strip_prefix(HEADS)
                        .and_then(|name| BranchName::parse(name).ok());
                }
                None => commit = CommitSha::parse(left).ok(),
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
    /// settings git chose for the file system and the repository's object and
    /// ref formats. The replacement is written beside the file under git's own
    /// lock name and renamed over it, so a git command running meanwhile reads
    /// either the old file or the new one, and one holding the lock is not
    /// overwritten.
    pub async fn reset_config(&self, path: &Path, url: &RepositoryUrl) -> GitResult<()> {
        let file = path.join(GIT_DIRECTORY).join(CONFIG_FILE);
        let listed = Self::output(
            Self::hardened()
                .args(["config", "--file"])
                .arg(&file)
                .args(["--list", "-z"])
                .stdout(Stdio::piped()),
        )
        .await?;
        let mut core = String::new();
        let mut extensions = String::new();
        if listed.status.success() {
            for entry in listed.stdout.split(|byte| *byte == 0) {
                let entry = String::from_utf8_lossy(entry);
                let Some((key, value)) = entry.split_once('\n') else {
                    continue;
                };
                if let Some(name) = key.strip_prefix("core.")
                    && CARRIED_CORE.contains(&name)
                    && matches!(value, "true" | "false")
                {
                    core.push_str(&format!("\t{name} = {value}\n"));
                } else if let Some(name) = key.strip_prefix("extensions.")
                    && CARRIED_EXTENSIONS
                        .iter()
                        .any(|(carried, values)| *carried == name && values.contains(&value))
                {
                    extensions.push_str(&format!("\t{name} = {value}\n"));
                }
            }
        }
        let (version, extensions) = match extensions.is_empty() {
            true => (0, extensions),
            false => (1, format!("[extensions]\n{extensions}")),
        };
        let content = format!(
            "[core]\n\trepositoryformatversion = {version}\n\tbare = false\n\tlogallrefupdates = true\n{core}{extensions}[remote \"{ORIGIN}\"]\n\turl = {}\n\tfetch = {FETCH_REFSPEC}\n",
            quoted(url.as_str())
        );
        replace_atomically(&file, &content).await
    }

    /// Whether the repository holds `commit`.
    pub async fn has_commit(&self, path: &Path, commit: &CommitSha) -> GitResult<bool> {
        Self::verify_config(path).await?;
        let output = Self::output(
            Self::hardened()
                .args(["cat-file", "-e", &format!("{commit}^{{commit}}")])
                .current_dir(path),
        )
        .await?;
        Ok(output.status.success())
    }

    /// Point `refs/remotes/origin/HEAD` at the remote's default branch, for
    /// whoever reads the ref; a run is started from the commit
    /// [`Self::remote_head`] reported, never from this ref.
    pub async fn set_remote_head(&self, path: &Path, branch: &BranchName) -> GitResult<()> {
        Self::verify_config(path).await?;
        Self::finish(
            Self::hardened()
                .args([
                    "symbolic-ref",
                    REMOTE_HEAD,
                    &format!("{REMOTE_TRACKING}{branch}"),
                ])
                .current_dir(path),
        )
        .await
    }

    /// Whether the working tree or the index holds anything uncommitted.
    pub async fn has_changes(&self, path: &Path) -> GitResult<bool> {
        Self::verify_config(path).await?;
        Ok(!Self::status(path).await?.is_empty())
    }

    /// Summarise the uncommitted changes, with the diff itself truncated
    /// once it runs past 50 kB.
    pub async fn diff_summary(&self, path: &Path) -> GitResult<DiffSummary> {
        Self::verify_config(path).await?;
        let files_changed = changed_paths(&Self::status(path).await?);

        let shortstat = Self::output(
            Self::hardened()
                .args([
                    "diff",
                    "--no-ext-diff",
                    "--no-textconv",
                    "--shortstat",
                    "HEAD",
                    "--",
                ])
                .current_dir(path)
                .stdout(Stdio::piped()),
        )
        .await?;

        let mut insertions = 0;
        let mut deletions = 0;

        if shortstat.status.success() {
            let statistics = String::from_utf8_lossy(&shortstat.stdout);
            for part in statistics.split(',') {
                let part = part.trim();
                let count = part
                    .split_whitespace()
                    .next()
                    .and_then(|count| count.parse().ok());
                if part.contains("insertion") {
                    insertions = count.unwrap_or(0);
                } else if part.contains("deletion") {
                    deletions = count.unwrap_or(0);
                }
            }
        }

        let diff = Self::output(
            Self::hardened()
                .args(["diff", "--no-ext-diff", "--no-textconv", "HEAD", "--"])
                .current_dir(path)
                .stdout(Stdio::piped()),
        )
        .await?;

        let diff_text = String::from_utf8_lossy(&diff.stdout);
        let diff_text = match diff_text.len() > MAXIMUM_DIFF_BYTES {
            true => {
                let mut end = MAXIMUM_DIFF_BYTES;
                while end > 0 && !diff_text.is_char_boundary(end) {
                    end -= 1;
                }
                format!("{}{TRUNCATED}", &diff_text[..end])
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

    /// `git status --porcelain -z` over every untracked file, whatever the
    /// repository's configuration says to show.
    async fn status(path: &Path) -> GitResult<Vec<u8>> {
        let output = Self::output(
            Self::hardened()
                .args(["status", "--porcelain", "-z", "--untracked-files=all"])
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

        Ok(output.stdout)
    }

    /// Create and check out a new branch, refusing a name already taken.
    pub async fn create_branch(&self, path: &Path, branch: &BranchName) -> GitResult<()> {
        Self::verify_config(path).await?;
        let check_output = Self::output(
            Self::hardened()
                .args(["show-ref", "--verify", "--quiet", &branch.reference()])
                .current_dir(path),
        )
        .await?;

        if check_output.status.success() {
            return Err(GitError::BranchExists(branch.clone()));
        }

        let output = Self::output(
            Self::hardened()
                .args(["checkout", "-b", branch.as_str(), "--"])
                .current_dir(path)
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
        Self::verify_config(path).await?;
        let output = Self::output(
            Self::hardened()
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
    pub async fn commit(&self, path: &Path, message: &str) -> GitResult<CommitSha> {
        Self::verify_config(path).await?;
        let staged = Self::output(
            Self::hardened()
                .args([
                    "diff",
                    "--cached",
                    "--quiet",
                    "--no-ext-diff",
                    "--no-textconv",
                ])
                .current_dir(path),
        )
        .await?;
        match staged.status.code() {
            Some(0) => return Err(GitError::NoChanges),
            Some(1) => {}
            _ => {
                return Err(GitError::CommandFailed(
                    "Cannot read the staged changes".to_string(),
                ));
            }
        }

        let output = Self::output(
            Self::hardened()
                .args([
                    "-c",
                    &format!("user.name={}", self.author_name),
                    "-c",
                    &format!("user.email={}", self.author_email),
                    "commit",
                    "-m",
                    message,
                ])
                .envs([
                    ("GIT_AUTHOR_NAME", &self.author_name),
                    ("GIT_AUTHOR_EMAIL", &self.author_email),
                    ("GIT_COMMITTER_NAME", &self.author_name),
                    ("GIT_COMMITTER_EMAIL", &self.author_email),
                ])
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

        self.revision(path, "HEAD").await
    }

    /// Push the checkout's HEAD to `branch` on the remote, with access token
    /// authentication, and say which commit was pushed. The commit is resolved
    /// before the push and the push names it rather than `HEAD`, so what the
    /// caller is told was pushed is what the remote received even if something
    /// moves HEAD meanwhile.
    pub async fn push_with_token(
        &self,
        path: &Path,
        branch: &BranchName,
        remote: &RepositoryUrl,
        token: &SecretValue,
    ) -> GitResult<CommitSha> {
        Self::verify_config(path).await?;
        let commit = self.revision(path, "HEAD").await?;
        let mut command = Self::connected(remote, Some(token));
        command
            .args([
                "push",
                "--porcelain",
                "--no-follow-tags",
                "--",
                remote.as_str(),
                &format!("{commit}:{}", branch.reference()),
            ])
            .current_dir(path)
            .stdout(Stdio::piped());
        let output = Self::output(&mut command).await?;
        if output.status.success() {
            let mut tracking = Self::hardened();
            tracking
                .args([
                    "update-ref",
                    &format!("{REMOTE_TRACKING}{branch}"),
                    commit.as_str(),
                ])
                .current_dir(path);
            if let Err(error) = Self::finish(&mut tracking).await {
                tracing::warn!(%error, "Pushed, but could not record the remote-tracking ref");
            }
            return Ok(commit);
        }
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
        Self::verify_config(path).await?;
        let output = Self::output(
            Self::hardened()
                .args(["remote", "get-url", "--", remote])
                .current_dir(path)
                .stdout(Stdio::piped()),
        )
        .await?;

        if !output.status.success() {
            return Err(GitError::NoRemote);
        }

        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    /// Switch to an existing local branch. Anything that is not one -- a tag,
    /// a commit, a remote branch of the same name, a file -- is refused rather
    /// than detached onto, tracked or restored.
    pub async fn checkout(&self, path: &Path, branch: &BranchName) -> GitResult<()> {
        Self::verify_config(path).await?;
        let output = Self::output(
            Self::hardened()
                .args(["switch", "--no-guess", "--", branch.as_str()])
                .current_dir(path)
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
}

/// The paths a `git status --porcelain -z` listing names. A rename or copy is
/// followed by the path it came from, which is not itself a change.
fn changed_paths(listing: &[u8]) -> Vec<String> {
    let mut fields = listing
        .split(|byte| *byte == 0)
        .filter(|field| !field.is_empty());
    let mut paths = Vec::new();
    while let Some(entry) = fields.next() {
        let Some((status, path)) = entry.split_at_checked(STATUS_WIDTH) else {
            continue;
        };
        paths.push(String::from_utf8_lossy(path).into_owned());
        if status.iter().any(|code| matches!(code, b'R' | b'C')) {
            fields.next();
        }
    }
    paths
}

/// A configuration value in double quotes, so nothing in it opens a comment
/// or a section.
fn quoted(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Replace `file` the way git does: write the whole new content to a lock
/// file beside it, created only if no git command already holds that lock,
/// and rename the lock file over the original.
async fn replace_atomically(file: &Path, content: &str) -> GitResult<()> {
    let lock = file.with_extension(LOCK_EXTENSION);
    let mut handle = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&lock)
        .await
        .map_err(|error| match error.kind() {
            std::io::ErrorKind::AlreadyExists => GitError::CommandFailed(
                "The repository's configuration is locked by another git command".to_string(),
            ),
            _ => GitError::Io(error),
        })?;
    let written = async {
        handle.write_all(content.as_bytes()).await?;
        handle.sync_all().await?;
        drop(handle);
        tokio::fs::rename(&lock, file).await
    }
    .await;
    if written.is_err() {
        let _ = tokio::fs::remove_file(&lock).await;
    }
    Ok(written?)
}

/// The NUL-separated names git prints under `-z`.
pub(super) fn split_nul(output: &[u8]) -> Vec<String> {
    output
        .split(|byte| *byte == 0)
        .filter(|name| !name.is_empty())
        .map(|name| String::from_utf8_lossy(name).into_owned())
        .collect()
}

#[cfg(test)]
pub(crate) mod fixtures {
    use super::*;

    pub(crate) fn local(path: &Path) -> RepositoryUrl {
        RepositoryUrl::local(path).unwrap()
    }

    pub(crate) fn branch(name: &str) -> BranchName {
        BranchName::parse(name).unwrap()
    }

    pub(crate) fn token() -> SecretValue {
        SecretValue::new("sensitive-token")
    }

    /// These fixtures share the machine with every other test binary, and under
    /// coverage instrumentation all of it is slower. The budgets only bound how
    /// long a genuine regression takes to surface, so they are generous.
    pub(crate) const SPAWN_BUDGET: Duration = Duration::from_secs(30);
    pub(crate) const TEARDOWN_BUDGET: Duration = Duration::from_secs(10);

    #[cfg(unix)]
    pub(crate) async fn marker(directory: &Path, name: &str) -> u32 {
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

    #[cfg(unix)]
    pub(crate) fn alive(pid: u32) -> bool {
        nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid as i32), None).is_ok()
    }
}

#[cfg(test)]
mod checkout_tests {
    use super::fixtures::local;
    use super::fixtures::token;
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
            GitService::repository_url("https://github.com/owner/repository")
                .unwrap()
                .as_str(),
            "https://github.com/owner/repository.git"
        );
    }

    #[test]
    fn authentication_is_not_in_arguments_or_repository_config() {
        let remote = RepositoryUrl::parse("https://github.com/owner/repository").unwrap();
        let command = GitService::connected(&remote, Some(&token()));
        let arguments = format!("{:?}", command.as_std().get_args().collect::<Vec<_>>());
        assert!(!arguments.contains("sensitive-token"));
        assert!(arguments.contains("credential.helper="));
        assert!(arguments.contains("http.followRedirects=false"));
    }

    #[test]
    fn a_credential_is_scoped_to_the_one_repository_it_was_given_for() {
        let remote = RepositoryUrl::parse("https://github.com/owner/repository").unwrap();
        let command = GitService::connected(&remote, Some(&token()));
        let environment: Vec<(String, String)> = command
            .as_std()
            .get_envs()
            .filter_map(|(key, value)| {
                Some((
                    key.to_string_lossy().into_owned(),
                    value?.to_string_lossy().into_owned(),
                ))
            })
            .collect();

        assert!(
            environment.contains(&(
                "GIT_CONFIG_KEY_0".to_string(),
                "http.https://github.com/owner/repository.git.extraHeader".to_string()
            )),
            "{environment:?}"
        );
        assert!(
            environment.contains(&("GIT_ALLOW_PROTOCOL".to_string(), "https".to_string())),
            "{environment:?}"
        );
        let listed = std::process::Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_COUNT", "1")
            .env(
                "GIT_CONFIG_KEY_0",
                "http.https://github.com/owner/repository.git.extraHeader",
            )
            .env("GIT_CONFIG_VALUE_0", "Authorization: fixture")
            .args(["config", "--get-urlmatch", "http.extraHeader"])
            .arg("https://github.com/owner/other.git/info/refs")
            .output()
            .unwrap();
        assert!(
            !listed.status.success(),
            "another repository on the same host must not match the header"
        );
    }

    #[tokio::test]
    async fn failed_clone_prevents_execution() {
        let fixture = tempfile::tempdir().unwrap();
        let mut executed = false;
        let result = async {
            GitService::new()
                .clone_repository(
                    &local(&fixture.path().join("missing")),
                    &fixture.path().join("checkout"),
                    Some(&token()),
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
            .clone_repository(&local(fixture.path()), &path, None)
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(path.join("sentinel")).unwrap(),
            "committed"
        );
        assert!(path.join(".git").is_dir());
        let second = destination.path().join("second");
        GitService::new()
            .clone_repository(&local(fixture.path()), &second, Some(&token()))
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
        let committed = service.commit(&path, "task change").await.unwrap();
        assert_eq!(
            committed.as_str(),
            crate::worktree::fixtures::git(&path, &["rev-parse", "HEAD"])
        );
        assert_eq!(
            crate::worktree::fixtures::git(&path, &["show", "-s", "--format=%an <%ae>"]),
            "Fixture <fixture@example.test>"
        );
    }
}

#[cfg(test)]
mod publication_tests {
    use super::fixtures::branch;
    use super::fixtures::local;
    use super::fixtures::token;
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
            git(
                fixture.path(),
                &["replace", "--graft", "HEAD", baseline.as_str()],
            );
        }
        let current = service.revision(fixture.path(), "HEAD").await.unwrap();
        assert!(
            !service
                .is_ancestor(fixture.path(), &baseline, &current)
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
        git(
            fixture.path(),
            &["replace", baseline.as_str(), replacement.as_str()],
        );
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
            .clone_repository(&local(&remote), &first, None)
            .await
            .unwrap();
        service
            .prepare_branch(&first, &branch("task/one"), false)
            .await
            .unwrap();
        std::fs::write(first.join("first"), "first attempt").unwrap();
        service.stage_all(&first).await.unwrap();
        let first_commit = service.commit(&first, "first attempt").await.unwrap();
        service
            .push_with_token(&first, &branch("task/one"), &local(&remote), &token())
            .await
            .unwrap();
        let second = fixture.path().join("second");
        service
            .clone_repository(&local(&remote), &second, None)
            .await
            .unwrap();
        service
            .prepare_branch(&second, &branch("task/one"), true)
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
            .push_with_token(&second, &branch("task/one"), &local(&remote), &token())
            .await
            .unwrap();
        assert_eq!(pushed, second_commit, "the push says which commit it sent");
        assert_eq!(
            git(&remote, &["rev-parse", "task/one"]),
            second_commit.as_str()
        );
        std::fs::write(first.join("concurrent"), "stale checkout").unwrap();
        service.stage_all(&first).await.unwrap();
        service.commit(&first, "concurrent attempt").await.unwrap();
        let failure = service
            .push_with_token(&first, &branch("task/one"), &local(&remote), &token())
            .await
            .unwrap_err()
            .to_string();
        assert!(failure.contains("Git push rejected"), "{failure}");
        assert!(
            !failure.contains("sensitive-token") && !failure.contains(remote.to_str().unwrap())
        );
        assert_eq!(
            git(&remote, &["rev-parse", "task/one"]),
            second_commit.as_str()
        );
        assert!(
            !std::fs::read_to_string(first.join(".git/config"))
                .unwrap()
                .contains("sensitive-token")
        );
        let current = service.revision(&second, "HEAD").await.unwrap();
        assert!(
            service
                .is_ancestor(&second, &first_commit, &current)
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
            .prepare_branch(&first, &branch("task/one"), false)
            .await
            .unwrap();
        assert_eq!(service.current_branch(&first).await.unwrap(), "task/one");
        assert!(
            service
                .prepare_branch(&first, &branch("task/one"), false)
                .await
                .is_ok(),
            "the worktree already on the branch is prepared again without complaint"
        );
        let second = root.path().join("second");
        crate::worktree::add(&base, &second, "origin/HEAD").unwrap();
        let refused = service
            .prepare_branch(&second, &branch("task/one"), false)
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
        service.fetch(&base, &local(&genuine), None).await.unwrap();
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
        service.fetch(&base, &local(&genuine), None).await.unwrap();
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
            .remote_head(&base, &local(&remote), None)
            .await
            .unwrap();
        assert_eq!(head.branch.as_str(), "main");
        assert_eq!(head.commit.as_str(), git(&remote, &["rev-parse", "main"]));
        assert!(service.has_commit(&base, &head.commit).await.unwrap());
        assert!(
            !service
                .has_commit(&base, &CommitSha::parse(&"0".repeat(40)).unwrap())
                .await
                .unwrap()
        );
        service.set_remote_head(&base, &head.branch).await.unwrap();
        assert_eq!(
            git(&base, &["symbolic-ref", "refs/remotes/origin/HEAD"]),
            "refs/remotes/origin/main"
        );
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
            .reset_config(
                &base,
                &RepositoryUrl::parse("https://github.com/fixture/repository.git").unwrap(),
            )
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
        assert!(RepositoryUrl::parse("https://github.com/x/y.git\n[core]").is_err());
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
            .prepare_branch(&first, &branch("task/one"), false)
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
            .prepare_branch(&second, &branch("task/one"), false)
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
            .prepare_branch(&third, &branch("task/other"), false)
            .await
            .unwrap();
        std::fs::remove_dir_all(&third).unwrap();
        let fourth = root.path().join("fourth");
        crate::worktree::add(&base, &fourth, "origin/HEAD").unwrap();
        service
            .prepare_branch(&fourth, &branch("task/other"), false)
            .await
            .unwrap();
        assert_eq!(service.current_branch(&fourth).await.unwrap(), "task/other");
    }
}

#[cfg(test)]
mod branch_tests {
    use super::fixtures::branch;
    use super::*;
    use crate::worktree::fixtures::git;
    use crate::worktree::fixtures::remote;

    #[tokio::test]
    async fn a_new_branch_is_created_and_checked_out_and_a_taken_name_is_refused() {
        let repository = tempfile::tempdir().unwrap();
        remote(repository.path());
        let service = GitService::new();

        service
            .create_branch(repository.path(), &branch("feature/one"))
            .await
            .unwrap();

        assert_eq!(
            service.current_branch(repository.path()).await.unwrap(),
            "feature/one"
        );
        assert!(matches!(
            service
                .create_branch(repository.path(), &branch("feature/one"))
                .await,
            Err(GitError::BranchExists(taken)) if taken == branch("feature/one")
        ));
    }

    #[tokio::test]
    async fn checking_out_switches_to_an_existing_branch() {
        let repository = tempfile::tempdir().unwrap();
        remote(repository.path());
        git(repository.path(), &["branch", "other"]);
        let service = GitService::new();

        service
            .checkout(repository.path(), &branch("other"))
            .await
            .unwrap();

        assert_eq!(
            service.current_branch(repository.path()).await.unwrap(),
            "other"
        );
    }

    /// A name that is a file rather than a branch used to restore the file,
    /// throwing away its uncommitted changes; a tag used to detach onto it.
    #[tokio::test]
    async fn checking_out_a_name_that_is_not_a_branch_changes_nothing() {
        let repository = tempfile::tempdir().unwrap();
        remote(repository.path());
        git(repository.path(), &["tag", "release"]);
        std::fs::write(repository.path().join("README"), "uncommitted\n").unwrap();
        let service = GitService::new();

        for name in ["README", "release", "missing"] {
            assert!(
                service
                    .checkout(repository.path(), &branch(name))
                    .await
                    .is_err(),
                "{name} is not a branch"
            );
        }

        assert_eq!(
            std::fs::read_to_string(repository.path().join("README")).unwrap(),
            "uncommitted\n"
        );
        assert_eq!(
            service.current_branch(repository.path()).await.unwrap(),
            "main"
        );
    }

    #[tokio::test]
    async fn a_remote_url_is_read_by_its_name_and_an_option_is_only_a_name() {
        let repository = tempfile::tempdir().unwrap();
        remote(repository.path());
        git(
            repository.path(),
            &[
                "remote",
                "add",
                "origin",
                "https://github.com/owner/repository.git",
            ],
        );
        let service = GitService::new();

        assert_eq!(
            service
                .get_remote_url(repository.path(), "origin")
                .await
                .unwrap(),
            "https://github.com/owner/repository.git"
        );
        for name in ["upstream", "--push", "--all"] {
            assert!(
                matches!(
                    service.get_remote_url(repository.path(), name).await,
                    Err(GitError::NoRemote)
                ),
                "{name}"
            );
        }
    }
}

#[cfg(test)]
mod configuration_tests {
    use super::fixtures::branch;
    use super::fixtures::local;
    use super::fixtures::token;
    use super::*;
    use crate::worktree::fixtures::clone;
    use crate::worktree::fixtures::git;
    use crate::worktree::fixtures::remote;

    fn repository() -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        remote(root.path());
        root
    }

    fn marking(marker: &Path) -> String {
        format!("touch '{}'; cat", marker.display())
    }

    #[tokio::test]
    async fn a_filter_driver_the_repository_names_never_runs() {
        let repository = repository();
        let markers = tempfile::tempdir().unwrap();
        let marker = markers.path().join("filter-ran");
        std::fs::write(
            repository.path().join(".gitattributes"),
            "* filter=planted\n",
        )
        .unwrap();
        git(
            repository.path(),
            &["config", "filter.planted.clean", &marking(&marker)],
        );
        std::fs::write(repository.path().join("new.txt"), "work\n").unwrap();

        let refusal = GitService::new().stage_all(repository.path()).await;

        assert!(
            matches!(refusal, Err(GitError::UnsafeConfig(ref key)) if key == "filter.planted.clean"),
            "{refusal:?}"
        );
        assert!(!marker.exists(), "the clean filter ran");
    }

    #[tokio::test]
    async fn a_diff_driver_the_repository_names_never_runs() {
        let repository = repository();
        let markers = tempfile::tempdir().unwrap();
        let marker = markers.path().join("textconv-ran");
        std::fs::write(repository.path().join(".gitattributes"), "* diff=planted\n").unwrap();
        git(
            repository.path(),
            &["config", "diff.planted.textconv", &marking(&marker)],
        );
        std::fs::write(repository.path().join("README"), "changed\n").unwrap();

        let refusal = GitService::new().diff_summary(repository.path()).await;

        assert!(
            matches!(refusal, Err(GitError::UnsafeConfig(ref key)) if key == "diff.planted.textconv"),
            "{refusal:?}"
        );
        assert!(!marker.exists(), "the textconv driver ran");
    }

    #[tokio::test]
    async fn a_transport_rewrite_in_the_clone_refuses_the_fetch_it_would_redirect() {
        let root = tempfile::tempdir().unwrap();
        let genuine = root.path().join("genuine");
        let rogue = root.path().join("rogue");
        for remote_path in [&genuine, &rogue] {
            std::fs::create_dir(remote_path).unwrap();
            remote(remote_path);
        }
        std::fs::write(rogue.join("ROGUE"), "planted\n").unwrap();
        git(&rogue, &["add", "ROGUE"]);
        git(&rogue, &["commit", "-q", "-m", "rogue"]);
        let base = root.path().join("base");
        clone(&genuine, &base);
        let before = git(&base, &["rev-parse", "refs/remotes/origin/main"]);
        git(
            &base,
            &[
                "config",
                &format!("url.{}.insteadOf", local(&rogue)),
                local(&genuine).as_str(),
            ],
        );

        let refusal = GitService::new().fetch(&base, &local(&genuine), None).await;

        assert!(
            matches!(refusal, Err(GitError::UnsafeConfig(ref key)) if key.starts_with("url.")),
            "{refusal:?}"
        );
        assert_eq!(
            git(&base, &["rev-parse", "refs/remotes/origin/main"]),
            before
        );
    }

    #[tokio::test]
    async fn a_remote_named_for_the_push_url_cannot_redirect_the_push() {
        let root = tempfile::tempdir().unwrap();
        let genuine = root.path().join("genuine.git");
        let rogue = root.path().join("rogue.git");
        let work = root.path().join("work");
        std::fs::create_dir(&work).unwrap();
        remote(&work);
        for bare in [&genuine, &rogue] {
            git(
                root.path(),
                &["init", "-q", "--bare", bare.to_str().unwrap()],
            );
        }
        git(
            &work,
            &[
                "config",
                &format!("remote.{}.pushurl", local(&genuine)),
                local(&rogue).as_str(),
            ],
        );

        let refusal = GitService::new()
            .push_with_token(&work, &branch("task/one"), &local(&genuine), &token())
            .await;

        assert!(
            matches!(refusal, Err(GitError::UnsafeConfig(ref key)) if key.starts_with("remote.")),
            "{refusal:?}"
        );
        assert_eq!(
            git(&rogue, &["for-each-ref"]),
            "",
            "the push reached the rogue"
        );
        assert_eq!(git(&genuine, &["for-each-ref"]), "");
    }

    #[tokio::test]
    async fn any_configuration_git_did_not_write_for_the_clone_refuses_it() {
        for (key, value) in [
            ("hook.planted.command", "true"),
            (
                "url.https://elsewhere.test/.insteadOf",
                "https://github.com/",
            ),
            ("http.proxy", "http://proxy.test"),
            ("http.https://github.com/.sslCAInfo", "/tmp/planted.pem"),
            ("include.path", "/tmp/planted.config"),
            ("includeIf.gitdir:/.path", "/tmp/planted.config"),
            ("filter.planted.smudge", "cat"),
            ("diff.external", "cat"),
            ("merge.planted.driver", "cat"),
            ("core.sshCommand", "ssh"),
            ("core.alternateRefsCommand", "cat"),
            ("core.worktree", "/tmp"),
            ("core.excludesFile", "/dev/null"),
            ("author.name", "Someone Else"),
            ("push.followTags", "true"),
            ("fetch.bundleURI", "https://elsewhere.test/bundle"),
            ("extensions.worktreeConfig", "true"),
            (
                "remote.https://github.com/owner/repository.git.pushurl",
                "x",
            ),
        ] {
            let repository = repository();
            git(repository.path(), &["config", key, value]);

            let refusal = GitService::new().current_branch(repository.path()).await;

            assert!(
                matches!(refusal, Err(GitError::UnsafeConfig(ref refused)) if *refused == key.to_ascii_lowercase()),
                "{key}: {refusal:?}"
            );
        }
    }

    /// Hooks defined in configuration run whatever `core.hooksPath` says,
    /// and see the credential of the command they run under; a repository
    /// defining one is refused by every hardened operation before anything
    /// runs in it.
    #[tokio::test]
    async fn a_hook_defined_in_configuration_never_runs() {
        let root = tempfile::tempdir().unwrap();
        let origin = root.path().join("origin");
        std::fs::create_dir(&origin).unwrap();
        remote(&origin);
        let base = root.path().join("base");
        clone(&origin, &base);
        let markers = tempfile::tempdir().unwrap();
        let marker = markers.path().join("hook-ran");
        git(
            &base,
            &[
                "config",
                "hook.planted.command",
                &format!("touch '{}'", marker.display()),
            ],
        );
        for event in [
            "reference-transaction",
            "post-index-change",
            "pre-push",
            "pre-commit",
            "post-commit",
        ] {
            git(&base, &["config", "--add", "hook.planted.event", event]);
        }
        std::fs::write(base.join("work.txt"), "work\n").unwrap();
        let service = GitService::new();
        let head = CommitSha::parse(&git(&base, &["rev-parse", "HEAD"])).unwrap();

        let refusals = [
            service.fetch(&base, &local(&origin), None).await.err(),
            service.has_changes(&base).await.err(),
            service.diff_summary(&base).await.err(),
            service.stage_all(&base).await.err(),
            service.commit(&base, "work").await.err(),
            service
                .create_branch(&base, &branch("task/one"))
                .await
                .err(),
            service
                .prepare_branch(&base, &branch("task/two"), false)
                .await
                .err(),
            service
                .push_with_token(&base, &branch("task/one"), &local(&origin), &token())
                .await
                .err(),
            service
                .remote_head(&base, &local(&origin), None)
                .await
                .err(),
            service.set_remote_head(&base, &branch("main")).await.err(),
            service.revision(&base, "HEAD").await.err(),
            service.has_commit(&base, &head).await.err(),
            service.is_ancestor(&base, &head, &head).await.err(),
            service.changed_files(&base, &head).await.err(),
            service.get_remote_url(&base, "origin").await.err(),
            service.checkout(&base, &branch("main")).await.err(),
        ];

        for (operation, refusal) in refusals.iter().enumerate() {
            assert!(
                matches!(refusal, Some(GitError::UnsafeConfig(key)) if key.starts_with("hook.")),
                "operation {operation}: {refusal:?}"
            );
        }
        assert!(!marker.exists(), "a configured hook ran");
    }

    #[tokio::test]
    async fn resetting_the_configuration_keeps_the_object_format_the_clone_was_made_in() {
        let root = tempfile::tempdir().unwrap();
        git(
            root.path(),
            &["init", "-q", "-b", "main", "--object-format=sha256"],
        );
        std::fs::write(root.path().join("README"), "fixture\n").unwrap();
        git(root.path(), &["add", "README"]);
        git(root.path(), &["commit", "-q", "-m", "fixture"]);
        let head = git(root.path(), &["rev-parse", "HEAD"]);
        let service = GitService::new();

        service
            .reset_config(
                root.path(),
                &RepositoryUrl::parse("https://github.com/owner/repository").unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(git(root.path(), &["rev-parse", "HEAD"]), head);
        assert_eq!(
            service
                .revision(root.path(), "HEAD")
                .await
                .unwrap()
                .as_str(),
            head
        );
        assert_eq!(
            git(root.path(), &["config", "extensions.objectformat"]),
            "sha256"
        );
        assert_eq!(
            git(root.path(), &["config", "remote.origin.url"]),
            "https://github.com/owner/repository.git"
        );
    }

    #[tokio::test]
    async fn a_configuration_another_git_command_holds_the_lock_on_is_not_overwritten() {
        let repository = repository();
        git(
            repository.path(),
            &["config", "core.fsmonitor", "/bin/false"],
        );
        let config = repository.path().join(".git").join("config");
        let before = std::fs::read_to_string(&config).unwrap();
        std::fs::write(config.with_extension("lock"), "held\n").unwrap();

        let refusal = GitService::new()
            .reset_config(
                repository.path(),
                &RepositoryUrl::parse("https://github.com/owner/repository").unwrap(),
            )
            .await;

        assert!(refusal.is_err(), "{refusal:?}");
        assert_eq!(std::fs::read_to_string(&config).unwrap(), before);
        assert_eq!(
            std::fs::read_to_string(config.with_extension("lock")).unwrap(),
            "held\n",
            "the lock belongs to whoever took it"
        );
    }

    #[tokio::test]
    async fn a_summary_names_every_changed_path_exactly_and_counts_the_lines() {
        let repository = repository();
        git(repository.path(), &["mv", "README", "RENAMED"]);
        std::fs::write(repository.path().join("RENAMED"), "fixture\nsecond\n").unwrap();
        std::fs::create_dir(repository.path().join("nested")).unwrap();
        std::fs::write(
            repository.path().join("nested").join("中文 name.txt"),
            "new\n",
        )
        .unwrap();
        std::fs::write(repository.path().join("quote\"d"), "new\n").unwrap();

        let summary = GitService::new()
            .diff_summary(repository.path())
            .await
            .unwrap();

        let mut files = summary.files_changed.clone();
        files.sort();
        assert_eq!(
            files,
            vec!["RENAMED", "nested/中文 name.txt", "quote\"d"],
            "{summary:?}"
        );
        assert_eq!(summary.insertions, 1, "{summary:?}");
        assert_eq!(summary.deletions, 0, "{summary:?}");
        assert!(summary.diff_text.contains("+second"), "{summary:?}");
        assert!(
            GitService::new()
                .has_changes(repository.path())
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn a_summary_past_its_limit_is_cut_on_a_character() {
        let repository = repository();
        std::fs::write(
            repository.path().join("README"),
            "é".repeat(MAXIMUM_DIFF_BYTES),
        )
        .unwrap();

        let summary = GitService::new()
            .diff_summary(repository.path())
            .await
            .unwrap();

        assert!(summary.diff_text.ends_with(TRUNCATED));
        assert!(summary.diff_text.len() <= MAXIMUM_DIFF_BYTES + TRUNCATED.len());
    }

    #[tokio::test]
    async fn an_untracked_file_is_a_change_and_a_clone_told_to_hide_them_is_refused() {
        let repository = repository();
        let service = GitService::new();
        assert!(!service.has_changes(repository.path()).await.unwrap());

        std::fs::write(repository.path().join("untracked"), "new\n").unwrap();
        assert!(service.has_changes(repository.path()).await.unwrap());

        git(
            repository.path(),
            &["config", "status.showUntrackedFiles", "no"],
        );
        assert!(matches!(
            service.has_changes(repository.path()).await,
            Err(GitError::UnsafeConfig(_))
        ));
    }
}

#[cfg(test)]
mod commit_tests {
    use super::*;
    use crate::worktree::fixtures::git;
    use crate::worktree::fixtures::remote;

    #[tokio::test]
    async fn a_commit_with_nothing_staged_reports_no_changes() {
        let repository = tempfile::tempdir().unwrap();
        remote(repository.path());
        std::fs::write(repository.path().join("unstaged"), "not added\n").unwrap();
        let service = GitService::new();

        let refusal = service.commit(repository.path(), "nothing").await;

        assert!(matches!(refusal, Err(GitError::NoChanges)), "{refusal:?}");
    }

    #[tokio::test]
    async fn a_repository_with_no_commits_and_nothing_staged_reports_no_changes() {
        let repository = tempfile::tempdir().unwrap();
        git(repository.path(), &["init", "-q", "-b", "main"]);

        let refusal = GitService::new().commit(repository.path(), "nothing").await;

        assert!(matches!(refusal, Err(GitError::NoChanges)), "{refusal:?}");
    }

    #[tokio::test]
    async fn the_first_commit_of_a_repository_is_made_and_named() {
        let repository = tempfile::tempdir().unwrap();
        git(repository.path(), &["init", "-q", "-b", "main"]);
        std::fs::write(repository.path().join("first"), "first\n").unwrap();
        let service = GitService::new();
        service.stage_all(repository.path()).await.unwrap();

        let committed = service.commit(repository.path(), "first").await.unwrap();

        assert_eq!(
            committed.as_str(),
            git(repository.path(), &["rev-parse", "HEAD"])
        );
    }
}

#[cfg(test)]
mod resumption_tests {
    use super::fixtures::branch;
    use super::*;
    use crate::worktree::fixtures::git;
    use crate::worktree::fixtures::remote;

    /// A tag sharing the branch's name made the checked-out branch read back
    /// as `heads/{name}`, and the worktree was refused as though another run
    /// held its own branch.
    #[tokio::test]
    async fn a_worktree_already_on_its_branch_is_resumed_whatever_else_shares_the_name() {
        let repository = tempfile::tempdir().unwrap();
        remote(repository.path());
        let service = GitService::new();
        service
            .prepare_branch(repository.path(), &branch("task/one"), false)
            .await
            .unwrap();
        git(repository.path(), &["tag", "task/one"]);

        service
            .prepare_branch(repository.path(), &branch("task/one"), false)
            .await
            .unwrap();

        assert_eq!(
            git(repository.path(), &["symbolic-ref", "HEAD"]),
            "refs/heads/task/one"
        );
    }
}
