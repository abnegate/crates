use crate::branch_name::BranchName;
use crate::commit_sha::CommitSha;
use crate::conflict::BASE_REF;
use crate::conflict::Conflict;
use crate::conflict::ConflictError;
use crate::conflict::ConflictRequest;
use crate::conflict::ConflictResult;
use crate::conflict::ConflictedPath;
use crate::conflict::HEAD_REF;
use crate::conflict::has_markers;
use crate::conflict::index;
use crate::conflict::layout::Layout;
use crate::conflict::resolve;
use crate::conflict::validate;
use crate::git::GitService;
use crate::repository_url::RepositoryUrl;
use abnegate_secret::SecretValue;
use std::collections::BTreeSet;
use std::ffi::OsString;
use std::process::Output;
use std::process::Stdio;
use tempfile::TempDir;
use tokio::process::Command;

/// The author a repair commits under when the caller names nobody.
const DEFAULT_AUTHOR_NAME: &str = "abnegate-vcs";

/// The address a repair commits under when the caller names nobody.
const DEFAULT_AUTHOR_EMAIL: &str = "abnegate-vcs@localhost";

/// How `git merge` reports the conflict it was asked to reproduce.
const MERGE_CONFLICTED: i32 = 1;

/// How `git push --porcelain` marks a ref the remote refused.
const REJECTED: &[u8] = b"!\t";

/// Reproduces pull request conflicts in throwaway checkouts.
#[derive(Debug, Clone)]
pub struct ConflictService {
    author_name: String,
    author_email: String,
}

impl Default for ConflictService {
    fn default() -> Self {
        Self::new()
    }
}

impl ConflictService {
    pub fn new() -> Self {
        Self {
            author_name: DEFAULT_AUTHOR_NAME.to_string(),
            author_email: DEFAULT_AUTHOR_EMAIL.to_string(),
        }
    }

    /// Commit a repair as `name <email>` rather than as the crate.
    #[must_use]
    pub fn with_author(mut self, name: impl Into<String>, email: impl Into<String>) -> Self {
        self.author_name = name.into();
        self.author_email = email.into();
        self
    }

    /// Fetch both sides into a fresh repository and merge them there.
    ///
    /// Fails with [`ConflictError::NoConflict`] when the merge succeeds, so a
    /// caller can treat a branch that no longer conflicts as nothing to do, and
    /// with [`ConflictError::Moved`] when either side has moved away from what
    /// the caller expected — a repair of a tree nobody asked about is worse than
    /// no repair at all.
    pub async fn reproduce(&self, request: &ConflictRequest) -> ConflictResult<Conflict> {
        let root = TempDir::new()?;
        let layout = Layout::under(root.path());
        layout.create()?;

        let mut init = self.command(&layout);
        init.args(["init", "--quiet", "--template=", "--separate-git-dir"])
            .arg(&layout.git)
            .arg(&layout.checkout);
        self.succeed(&mut init, "init").await?;

        let mut fetch = self.bound(&layout);
        GitService::connect(&mut fetch, &request.remote, request.token.as_ref());
        fetch.args(fetch_arguments(
            &request.remote,
            &request.head,
            &request.base,
        ));
        self.succeed(&mut fetch, "fetch").await?;

        let head = self.rev_parse(&layout, HEAD_REF).await?;
        let base = self.rev_parse(&layout, BASE_REF).await?;
        expect(&request.expected_head, &head, &request.head)?;
        expect(&request.expected_base, &base, &request.base)?;

        self.run(&layout, &["checkout", "--detach", "--quiet", HEAD_REF])
            .await?;

        let merged = self
            .attempt(&layout, &["merge", "--no-commit", "--no-ff", BASE_REF])
            .await?;

        let unmerged = self
            .capture(
                &layout,
                &[
                    "diff",
                    "--no-ext-diff",
                    "--no-textconv",
                    "--name-only",
                    "--diff-filter=U",
                    "-z",
                ],
            )
            .await?;

        let mut files = BTreeSet::new();
        for entry in unmerged.split('\0').filter(|entry| !entry.is_empty()) {
            files.insert(ConflictedPath::parse(entry)?);
        }
        let files: Vec<ConflictedPath> = files.into_iter().collect();

        if merged || files.is_empty() {
            return Err(ConflictError::NoConflict);
        }

        for path in &files {
            let resolved = resolve(&layout.checkout, path)?;
            let text = std::fs::read_to_string(&resolved)
                .map_err(|_| ConflictError::NotTextual(path.to_string()))?;
            if !has_markers(&text) {
                return Err(ConflictError::NotTextual(path.to_string()));
            }
        }

        let index = self
            .capture(&layout, &["ls-files", "--stage", "-z"])
            .await?;

        Ok(Conflict {
            root,
            layout,
            remote: request.remote.clone(),
            head_branch: request.head.clone(),
            head,
            base,
            files,
            index,
        })
    }

    /// Files the repair changed that the conflict did not name: an edit the
    /// merge did not leave there, anything it staged or unstaged itself, and
    /// any file it created that the repository does not ignore.
    ///
    /// Only the conflicted files have any business being in that list.
    pub async fn strays(&self, conflict: &Conflict) -> ConflictResult<Vec<String>> {
        self.verify_config(&conflict.layout).await?;
        let staged = self
            .capture(&conflict.layout, &["ls-files", "--stage", "-z"])
            .await?;
        let modified = self
            .capture(
                &conflict.layout,
                &[
                    "diff",
                    "--no-ext-diff",
                    "--no-textconv",
                    "--name-only",
                    "-z",
                ],
            )
            .await?;
        let created = self
            .capture(
                &conflict.layout,
                &["ls-files", "--others", "--exclude-standard", "-z"],
            )
            .await?;

        let named: BTreeSet<&str> = conflict.files().iter().map(|path| path.as_str()).collect();
        let mut strays = index::changed(&conflict.index, &staged);
        strays.extend(
            modified
                .split('\0')
                .chain(created.split('\0'))
                .filter(|entry| !entry.is_empty())
                .map(str::to_string),
        );
        strays.retain(|path| !named.contains(path.as_str()));
        Ok(strays.into_iter().collect())
    }

    /// Commit the resolved merge, staging only the conflicted files.
    ///
    /// The checkout is verified first, and a repair that touched anything the
    /// conflict did not name is refused with [`ConflictError::Strays`]: the
    /// merge staged everything that combined cleanly, so adding the conflicted
    /// files completes the index, and nothing else may be in it. Each
    /// conflicted file must still be a regular file inside the checkout, so a
    /// repair cannot commit a link in its place.
    pub async fn apply(&self, conflict: &Conflict, message: &str) -> ConflictResult<CommitSha> {
        conflict.verify(self).await?;
        let strays = self.strays(conflict).await?;
        if !strays.is_empty() {
            return Err(ConflictError::Strays(strays));
        }
        validate(conflict.path(), conflict.files())?;

        let mut arguments: Vec<&str> = vec!["add", "--"];
        arguments.extend(conflict.files().iter().map(|path| path.as_str()));
        self.run(&conflict.layout, &arguments).await?;

        let name = format!("user.name={}", self.author_name);
        let email = format!("user.email={}", self.author_email);
        self.run(
            &conflict.layout,
            &[
                "-c",
                &name,
                "-c",
                &email,
                "commit",
                "--no-verify",
                "--quiet",
                "-m",
                message,
            ],
        )
        .await?;

        self.rev_parse(&conflict.layout, "HEAD").await
    }

    /// Push the applied repair to the branch and repository it was reproduced
    /// from, refusing anything that is not a fast-forward, and say which
    /// commit was pushed.
    ///
    /// Only a merge of the reproduced head and base is pushed: a checkout
    /// whose HEAD is anything else was not repaired by [`Self::apply`]. A
    /// branch somebody else advanced during the repair rejects the push rather
    /// than losing their commits, because nothing here ever forces.
    pub async fn publish(
        &self,
        conflict: &Conflict,
        token: Option<&SecretValue>,
    ) -> ConflictResult<CommitSha> {
        self.verify_config(&conflict.layout).await?;
        let lineage = self
            .capture(
                &conflict.layout,
                &["rev-list", "--parents", "--max-count=1", "HEAD", "--"],
            )
            .await?;
        let lineage = lineage
            .split_whitespace()
            .map(CommitSha::parse)
            .collect::<Result<Vec<CommitSha>, _>>()?;
        let [commit, first, second] = lineage.as_slice() else {
            return Err(ConflictError::NotApplied);
        };
        if *first != conflict.head || *second != conflict.base {
            return Err(ConflictError::NotApplied);
        }
        let commit = commit.clone();

        let mut push = self.bound(&conflict.layout);
        GitService::connect(&mut push, &conflict.remote, token);
        push.args(push_arguments(
            &conflict.remote,
            &commit,
            &conflict.head_branch,
        ));
        let output = self.execute(&mut push).await?;
        if output.status.success() {
            return Ok(commit);
        }
        let rejected = output
            .stdout
            .split(|byte| *byte == b'\n')
            .any(|line| line.starts_with(REJECTED));
        match rejected {
            true => Err(ConflictError::Rejected),
            false => Err(ConflictError::CommandFailed(
                "git push failed; verify repository access".to_string(),
            )),
        }
    }

    pub(super) async fn rev_parse(
        &self,
        layout: &Layout,
        reference: &str,
    ) -> ConflictResult<CommitSha> {
        let output = self
            .capture(
                layout,
                &[
                    "rev-parse",
                    "--verify",
                    "--end-of-options",
                    &format!("{reference}^{{commit}}"),
                ],
            )
            .await?;
        Ok(CommitSha::parse(&output)?)
    }

    /// Refuse a repository whose configuration names something no pin reaches.
    pub(super) async fn verify_config(&self, layout: &Layout) -> ConflictResult<()> {
        Ok(GitService::verify(&mut self.bound(layout)).await?)
    }

    async fn run(&self, layout: &Layout, arguments: &[&str]) -> ConflictResult<()> {
        self.capture(layout, arguments).await.map(drop)
    }

    async fn capture(&self, layout: &Layout, arguments: &[&str]) -> ConflictResult<String> {
        let mut command = self.bound(layout);
        command.args(arguments);
        let output = self.succeed(&mut command, arguments[0]).await?;
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    /// Run a command whose failure is an error, reported by the operation's
    /// name alone: git's own output quotes paths and remote messages the
    /// repository controls.
    async fn succeed(&self, command: &mut Command, operation: &str) -> ConflictResult<Output> {
        let output = self.execute(command).await?;
        if !output.status.success() {
            tracing::debug!(
                operation,
                error = %String::from_utf8_lossy(&output.stderr),
                "A conflict repair's git command failed"
            );
            return Err(ConflictError::CommandFailed(format!(
                "git {operation} failed"
            )));
        }
        Ok(output)
    }

    /// Run a merge whose failure is an answer rather than an error.
    ///
    /// Exit 1 is the conflict git was asked to produce. Any other failure —
    /// a missing ref, an unusable identity, a broken checkout — is an error,
    /// and reporting it as "merged cleanly" turns a broken environment into a
    /// silent no-op that looks exactly like a branch needing no repair.
    async fn attempt(&self, layout: &Layout, arguments: &[&str]) -> ConflictResult<bool> {
        let mut command = self.bound(layout);
        command.args(arguments);
        let output = self.execute(&mut command).await?;
        if output.status.success() {
            return Ok(true);
        }
        if output.status.code() == Some(MERGE_CONFLICTED) {
            return Ok(false);
        }
        Err(ConflictError::CommandFailed(format!(
            "git {} exited with {}",
            arguments[0],
            output
                .status
                .code()
                .map_or_else(|| "a signal".to_string(), |code| code.to_string())
        )))
    }

    /// Every command runs under the git service's timeout, in a process group
    /// torn down with it when it is abandoned.
    async fn execute(&self, command: &mut Command) -> ConflictResult<Output> {
        Ok(GitService::output(command).await?)
    }

    /// A hardened command bound to the throwaway repository and work tree by
    /// path, so nothing in the checkout decides which repository it reads.
    fn bound(&self, layout: &Layout) -> Command {
        let mut command = self.command(layout);
        command
            .arg(prefixed("--git-dir=", &layout.git))
            .arg(prefixed("--work-tree=", &layout.checkout));
        command
    }

    /// A hardened command with the repair's identity and an empty home
    /// outside everything the repair can write.
    fn command(&self, layout: &Layout) -> Command {
        let mut command = GitService::hardened();
        command
            .current_dir(&layout.checkout)
            .env("HOME", &layout.home)
            .env("GIT_AUTHOR_NAME", &self.author_name)
            .env("GIT_AUTHOR_EMAIL", &self.author_email)
            .env("GIT_COMMITTER_NAME", &self.author_name)
            .env("GIT_COMMITTER_EMAIL", &self.author_email)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }
}

/// `git fetch` of exactly the two branches, into refs of the throwaway
/// repository's own, with the remote after the end of the options.
fn fetch_arguments(remote: &RepositoryUrl, head: &BranchName, base: &BranchName) -> Vec<String> {
    vec![
        "fetch".to_string(),
        "--no-tags".to_string(),
        "--quiet".to_string(),
        "--".to_string(),
        remote.to_string(),
        format!("+{}:{HEAD_REF}", head.reference()),
        format!("+{}:{BASE_REF}", base.reference()),
    ]
}

/// `git push` of one commit to one branch, never forced, with the remote
/// after the end of the options.
fn push_arguments(remote: &RepositoryUrl, commit: &CommitSha, branch: &BranchName) -> Vec<String> {
    vec![
        "push".to_string(),
        "--porcelain".to_string(),
        "--no-follow-tags".to_string(),
        "--".to_string(),
        remote.to_string(),
        format!("{commit}:{}", branch.reference()),
    ]
}

fn prefixed(option: &str, path: &std::path::Path) -> OsString {
    let mut argument = OsString::from(option);
    argument.push(path);
    argument
}

fn expect(
    expected: &Option<CommitSha>,
    actual: &CommitSha,
    branch: &BranchName,
) -> ConflictResult<()> {
    match expected {
        Some(expected) if expected != actual => Err(ConflictError::Moved {
            branch: branch.clone(),
            expected: expected.clone(),
            actual: actual.clone(),
        }),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn environment(command: &Command) -> Vec<(String, String)> {
        command
            .as_std()
            .get_envs()
            .filter_map(|(key, value)| {
                Some((
                    key.to_string_lossy().into_owned(),
                    value?.to_string_lossy().into_owned(),
                ))
            })
            .collect()
    }

    fn remote() -> RepositoryUrl {
        RepositoryUrl::parse("https://github.com/owner/repository").unwrap()
    }

    /// The identity a repair commits under reaches git through the environment
    /// as well as the command line, so a caller that named one is obeyed by
    /// both the merge and the commit.
    #[test]
    fn the_author_a_repair_commits_under_is_the_callers_to_name() {
        let root = TempDir::new().unwrap();
        let service = ConflictService::new().with_author("Ada", "ada@example.test");
        let environment = environment(&service.command(&Layout::under(root.path())));

        assert!(
            environment.contains(&("GIT_AUTHOR_NAME".to_string(), "Ada".to_string())),
            "{environment:?}"
        );
        assert!(
            environment.contains(&(
                "GIT_COMMITTER_EMAIL".to_string(),
                "ada@example.test".to_string()
            )),
            "{environment:?}"
        );
    }

    #[test]
    fn every_command_is_hardened_and_bound_to_a_repository_outside_the_checkout() {
        let root = TempDir::new().unwrap();
        let layout = Layout::under(root.path());
        let command = ConflictService::new().bound(&layout);
        let arguments: Vec<String> = command
            .as_std()
            .get_args()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect();
        let environment = environment(&command);

        assert!(arguments.contains(&format!("--git-dir={}", layout.git.display())));
        assert!(arguments.contains(&format!("--work-tree={}", layout.checkout.display())));
        assert!(arguments.contains(&"core.hooksPath=/dev/null".to_string()));
        assert!(!layout.git.starts_with(&layout.checkout));
        for expected in [
            ("HOME", layout.home.display().to_string()),
            ("GIT_ALLOW_PROTOCOL", "https".to_string()),
            ("GIT_LITERAL_PATHSPECS", "1".to_string()),
            ("GIT_CONFIG_GLOBAL", "/dev/null".to_string()),
        ] {
            assert!(
                environment.contains(&(expected.0.to_string(), expected.1.clone())),
                "{expected:?} in {environment:?}"
            );
        }
        assert!(!layout.home.starts_with(&layout.checkout));
        assert!(!layout.home.starts_with(&layout.isolation));
    }

    #[test]
    fn the_remote_comes_after_the_end_of_the_options() {
        let head = BranchName::parse("feature").unwrap();
        let base = BranchName::parse("main").unwrap();
        let commit = CommitSha::parse(&"a".repeat(40)).unwrap();

        for arguments in [
            fetch_arguments(&remote(), &head, &base),
            push_arguments(&remote(), &commit, &head),
        ] {
            let end = arguments.iter().position(|argument| argument == "--");
            let remote = arguments
                .iter()
                .position(|argument| argument == "https://github.com/owner/repository.git");
            assert!(
                matches!((end, remote), (Some(end), Some(remote)) if end < remote),
                "{arguments:?}"
            );
        }
        assert_eq!(
            push_arguments(&remote(), &commit, &head).last().unwrap(),
            &format!("{commit}:refs/heads/feature")
        );
    }

    #[test]
    fn a_request_never_prints_its_token() {
        let request = ConflictRequest {
            remote: remote(),
            token: Some(SecretValue::new(concat!("ghp_", "sensitive"))),
            head: BranchName::parse("feature").unwrap(),
            base: BranchName::parse("main").unwrap(),
            expected_head: None,
            expected_base: None,
        };

        assert!(!format!("{request:?}").contains(concat!("ghp_", "sensitive")));
    }

    /// Abandoning a repair's git command takes every helper it started with
    /// it, as abandoning one of the git service's own does.
    #[cfg(unix)]
    #[tokio::test]
    async fn an_abandoned_command_takes_its_helpers_with_it() {
        use crate::git::fixtures::TEARDOWN_BUDGET;
        use crate::git::fixtures::alive;
        use crate::git::fixtures::marker;
        use std::os::unix::fs::PermissionsExt;

        let fixture = TempDir::new().unwrap();
        std::fs::write(
            fixture.path().join("git"),
            "#!/bin/sh\n/bin/sleep 60 &\necho $! > \"$FIXTURE/helper\"\nwait\n",
        )
        .unwrap();
        std::fs::set_permissions(
            fixture.path().join("git"),
            std::fs::Permissions::from_mode(0o700),
        )
        .unwrap();
        let root = TempDir::new().unwrap();
        let layout = Layout::under(root.path());
        layout.create().unwrap();
        let service = ConflictService::new();
        let mut command = service.bound(&layout);
        command
            .env("PATH", fixture.path())
            .env("FIXTURE", fixture.path());

        let operation = tokio::spawn(async move { service.execute(&mut command).await });
        let helper = marker(fixture.path(), "helper").await;
        operation.abort();
        assert!(operation.await.unwrap_err().is_cancelled());
        let stopped = tokio::time::timeout(TEARDOWN_BUDGET, async {
            while alive(helper) {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .is_ok();
        let _ = nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(helper as i32),
            nix::sys::signal::Signal::SIGKILL,
        );

        assert!(stopped, "a helper outlived the abandoned repair command");
    }
}
