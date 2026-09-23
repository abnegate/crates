use crate::branch_name::BranchName;
use crate::branch_name::HEADS;
use crate::commit_sha::CommitSha;
use crate::git::CONFIG_LISTING;
use crate::git::DIFF_PREFIX;
use crate::git::DiffSummary;
use crate::git::GitError;
use crate::git::GitResult;
use crate::git::IGNORE_SUBMODULES;
use crate::git::PINS;
use crate::git::RemoteHead;
use crate::git::authentication::authenticate;
#[cfg(unix)]
use crate::git::group::Group;
use crate::git::harden;
use crate::git::refused;
use crate::repository_url::RepositoryUrl;
use abnegate_secret::SecretValue;
use std::ffi::OsStr;
use std::ffi::OsString;
use std::path::Path;
use std::path::PathBuf;
use std::process::Output;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use uuid::Uuid;

mod hardened;
mod managed;
mod worktrees;

#[cfg(test)]
pub(crate) use hardened::fixtures;

/// Longest diff kept before truncation, in bytes.
const MAXIMUM_DIFF_BYTES: usize = 50_000;

/// Longest a generated branch name may run, in bytes.
const MAXIMUM_BRANCH_LENGTH: usize = 100;

/// Characters of a task's title a generated branch name is made from.
const MAXIMUM_TITLE_CHARACTERS: usize = 50;

/// Hex digits of a task's identifier a generated branch name carries.
const SHORT_IDENTIFIER_LENGTH: usize = 8;

/// Characters a cut branch name may not end on.
const TRAILING: [char; 3] = ['-', '/', '.'];

/// Longest a git command may run before it is torn down.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(300);

/// How long a fetch refused by a ref lock waits before its one retry.
const RETRY_DELAY: Duration = Duration::from_millis(500);

/// The branch namespace [`GitService::generate_branch_name`] works in.
const DEFAULT_BRANCH_PREFIX: &str = "task";

/// The author a commit is made under when the caller names nobody.
const DEFAULT_AUTHOR_NAME: &str = "abnegate-vcs";

/// The address a commit is made under when the caller names nobody.
const DEFAULT_AUTHOR_EMAIL: &str = "abnegate-vcs@localhost";

/// The remote every clone this service makes or manages calls its source.
const ORIGIN: &str = "origin";

/// The namespace the remote's branches are tracked under.
const REMOTE_TRACKING: &str = "refs/remotes/origin/";

/// The ref naming the remote's default branch.
const REMOTE_HEAD: &str = "refs/remotes/origin/HEAD";

/// Every branch the remote has, tracked under [`REMOTE_TRACKING`].
const FETCH_REFSPEC: &str = "+refs/heads/*:refs/remotes/origin/*";

/// Has a fetch write only what the refspecs on its command line name, and
/// nothing a remote's configured refspecs would also map a fetched ref to.
const IGNORE_CONFIGURED_REFSPECS: &str = "--refmap=";

/// How `ls-remote --symref` introduces the ref a symbolic ref points at.
const SYMBOLIC_REFERENCE: &str = "ref: ";

/// What a set-aside branch is renamed with, ahead of the time it was set aside.
const ABANDONED: &str = ".abandoned.";

/// What a diff cut at [`MAXIMUM_DIFF_BYTES`] ends with.
const TRUNCATED: &str = "...[truncated]";

/// The status letters and the space before a path in a porcelain listing.
const STATUS_WIDTH: usize = 3;

/// The directory a clone keeps its repository in.
const GIT_DIRECTORY: &str = ".git";

/// A repository's own configuration file.
const CONFIG_FILE: &str = "config";

/// What git names the file it writes a replacement into before renaming it.
const LOCK_EXTENSION: &str = "lock";

/// Core settings git chose for the file system when it made the clone.
const CARRIED_CORE: [&str; 4] = ["filemode", "ignorecase", "precomposeunicode", "symlinks"];

/// Pinned settings a fetch into a managed clone leaves to the caller's own
/// configuration, because pinning them would blank the credential helper and
/// proxy the caller set up.
const LEFT_TO_CALLER: [&str; 2] = ["credential.helper=", "http.proxy="];

/// The transports a managed clone is cloned and fetched over: a local path,
/// HTTPS and SSH. Plain HTTP, `git://` and every remote helper are refused.
const MANAGED_PROTOCOLS: &str = "file:https:ssh";

/// The key naming the address a managed clone fetches from.
const ORIGIN_URL: &str = "remote.origin.url";

/// The key naming which of the remote's refs a fetch writes, and where.
const ORIGIN_FETCH: &str = "remote.origin.fetch";

/// Lists every value the key that follows holds in a repository's own
/// configuration, each ended by a NUL.
const VALUE_LISTING: [&str; 4] = ["config", "--local", "-z", "--get-all"];

/// Repository formats a clone was made in, and the values each may take:
/// without them git cannot read its own objects or refs.
const CARRIED_EXTENSIONS: [(&str, &[&str]); 2] = [
    ("objectformat", &["sha1", "sha256"]),
    ("refstorage", &["files", "reftable"]),
];

/// Git operations against a repository on disk.
#[derive(Debug, Clone)]
pub struct GitService {
    maximum_branch_length: usize,
    branch_prefix: BranchName,
    author_name: String,
    author_email: String,
}

impl Default for GitService {
    fn default() -> Self {
        Self::new()
    }
}

impl GitService {
    pub fn new() -> Self {
        Self {
            maximum_branch_length: MAXIMUM_BRANCH_LENGTH,
            branch_prefix: BranchName::literal(DEFAULT_BRANCH_PREFIX),
            author_name: DEFAULT_AUTHOR_NAME.to_string(),
            author_email: DEFAULT_AUTHOR_EMAIL.to_string(),
        }
    }

    /// Name generated branches under `prefix` rather than `task`.
    #[must_use]
    pub fn with_branch_prefix(mut self, prefix: BranchName) -> Self {
        self.branch_prefix = prefix;
        self
    }

    /// Commit as `name <email>` rather than as the crate.
    #[must_use]
    pub fn with_author(mut self, name: impl Into<String>, email: impl Into<String>) -> Self {
        self.author_name = name.into();
        self.author_email = email.into();
        self
    }

    /// Cut generated branch names at `length` bytes. The prefix and the task's
    /// identifier are never cut: a name that could not tell two tasks apart is
    /// no name at all.
    #[must_use]
    pub fn with_maximum_branch_length(mut self, length: usize) -> Self {
        self.maximum_branch_length = length;
        self
    }

    /// Accept GitHub HTTPS repositories without URL credentials or transport options.
    pub fn repository_url(source: &str) -> GitResult<RepositoryUrl> {
        Ok(RepositoryUrl::parse(source)?)
    }

    /// Name a branch after the task it carries: `{prefix}/{short id}-{slug}`,
    /// where the slug is the title's ASCII letters and digits, lowercased, and
    /// whatever else it holds is one hyphen between them.
    pub fn generate_branch_name(&self, task_id: Uuid, title: &str) -> GitResult<BranchName> {
        let identifier = task_id.simple().to_string();
        let stem = format!(
            "{}/{}",
            self.branch_prefix,
            &identifier[..SHORT_IDENTIFIER_LENGTH]
        );
        let slug = slug(title);
        let candidate = match slug.is_empty() {
            true => stem.clone(),
            false => format!("{stem}-{slug}"),
        };
        let mut end = candidate
            .len()
            .min(self.maximum_branch_length.max(stem.len()));
        while !candidate.is_char_boundary(end) {
            end -= 1;
        }
        Ok(BranchName::parse(
            candidate[..end].trim_end_matches(TRAILING),
        )?)
    }

    /// A git invocation that reads nothing from the host's configuration and
    /// runs no program the repository's configuration names.
    pub(crate) fn hardened() -> Command {
        let mut command = Command::new("git");
        harden(command.as_std_mut());
        command
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        command
    }

    /// Refuse a repository whose own configuration holds anything beyond what
    /// git writes for a clone, a worktree and a tracking branch. Run before
    /// every hardened operation, because a run's git commands can write that
    /// configuration between two of them.
    pub(crate) async fn verify_config(path: &Path) -> GitResult<()> {
        Self::verify(Self::hardened().current_dir(path)).await
    }

    /// [`Self::verify_config`] for a command already pointed at its repository.
    pub(crate) async fn verify(command: &mut Command) -> GitResult<()> {
        let listed = Self::output(command.args(CONFIG_LISTING).stdout(Stdio::piped())).await?;
        if !listed.status.success() {
            return Err(GitError::CommandFailed(
                "Cannot read the repository's configuration".to_string(),
            ));
        }
        match refused(&listed.stdout) {
            Some(key) => Err(GitError::UnsafeConfig(key)),
            None => Ok(()),
        }
    }

    /// A hardened invocation that may reach `remote`, over the one transport
    /// it names, with `token` sent to that repository and nowhere else.
    pub(crate) fn connected(remote: &RepositoryUrl, token: Option<&SecretValue>) -> Command {
        let mut command = Self::hardened();
        Self::connect(&mut command, remote, token);
        command
    }

    /// Let a hardened command reach `remote` and nothing else.
    pub(crate) fn connect(
        command: &mut Command,
        remote: &RepositoryUrl,
        token: Option<&SecretValue>,
    ) {
        command.env("GIT_ALLOW_PROTOCOL", remote.protocol());
        if let Some(token) = token {
            authenticate(command, remote, token);
        }
    }

    /// Run a git command in a process group of its own, torn down with every
    /// helper it started if it outlives [`COMMAND_TIMEOUT`] or its caller.
    pub(crate) async fn output(command: &mut Command) -> GitResult<Output> {
        #[cfg(test)]
        fixtures::record(command);
        #[cfg(unix)]
        command.process_group(0);
        command.kill_on_drop(true);
        let child = command.spawn()?;
        #[cfg(unix)]
        let mut group = Group::new(nix::unistd::Pid::from_raw(
            child
                .id()
                .ok_or_else(|| std::io::Error::other("Git process has no ID"))? as i32,
        ));
        let output = tokio::time::timeout(COMMAND_TIMEOUT, child.wait_with_output())
            .await
            .map_err(|_| GitError::TimedOut)??;
        #[cfg(unix)]
        group.disarm();
        Ok(output)
    }

    /// Run a git command as [`Self::output`] does, but read at most `cap + 1`
    /// bytes of its standard output and tear the process group down the instant
    /// that much has arrived, so a run cannot make the host hold an unbounded
    /// diff or listing in memory. The second value is whether the output ran
    /// past `cap`.
    pub(crate) async fn capped(command: &mut Command, cap: usize) -> GitResult<(Vec<u8>, bool)> {
        #[cfg(unix)]
        command.process_group(0);
        command.kill_on_drop(true).stdout(Stdio::piped());
        let mut child = command.spawn()?;
        #[cfg(unix)]
        let mut group = Group::new(nix::unistd::Pid::from_raw(
            child
                .id()
                .ok_or_else(|| std::io::Error::other("Git process has no ID"))? as i32,
        ));
        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| std::io::Error::other("Git process has no standard output"))?;
        let ceiling = cap.saturating_add(1);
        let mut collected = Vec::new();
        let read = tokio::time::timeout(COMMAND_TIMEOUT, async {
            let mut chunk = [0u8; 8192];
            loop {
                let read = stdout.read(&mut chunk).await?;
                if read == 0 {
                    break;
                }
                let room = ceiling - collected.len();
                collected.extend_from_slice(&chunk[..read.min(room)]);
                if collected.len() >= ceiling {
                    break;
                }
            }
            Ok::<(), std::io::Error>(())
        })
        .await;
        let truncated = collected.len() > cap;
        match read {
            Ok(Ok(())) if !truncated => {
                let waited = tokio::time::timeout(COMMAND_TIMEOUT, child.wait())
                    .await
                    .map_err(|_| GitError::TimedOut)?;
                waited?;
                #[cfg(unix)]
                group.disarm();
                Ok((collected, false))
            }
            Ok(Ok(())) => Ok((collected, true)),
            Ok(Err(error)) => Err(error.into()),
            Err(_) => Err(GitError::TimedOut),
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
}

/// A title's ASCII letters and digits, lowercased, with every run of anything
/// else between them one hyphen.
fn slug(title: &str) -> String {
    title
        .chars()
        .take(MAXIMUM_TITLE_CHARACTERS)
        .map(|character| match character.is_ascii_alphanumeric() {
            true => character.to_ascii_lowercase(),
            false => '-',
        })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<&str>>()
        .join("-")
}

#[cfg(test)]
mod branch_name_tests {
    use super::*;

    fn task() -> Uuid {
        Uuid::parse_str("12345678-1234-1234-1234-123456789abc").unwrap()
    }

    fn generate(service: &GitService, title: &str) -> String {
        service
            .generate_branch_name(task(), title)
            .unwrap()
            .to_string()
    }

    #[test]
    fn a_branch_is_named_after_the_task_and_its_title() {
        let branch = generate(&GitService::new(), "Fix the login bug");

        assert_eq!(branch, "task/12345678-fix-the-login-bug");
    }

    #[test]
    fn anything_a_branch_name_may_not_carry_becomes_a_single_hyphen() {
        let branch = generate(&GitService::new(), "Add user@email validation!!!");

        assert_eq!(branch, "task/12345678-add-user-email-validation");
    }

    #[test]
    fn a_title_longer_than_the_name_allows_is_cut_to_it() {
        let branch = generate(&GitService::new(), &"A".repeat(200));

        assert!(branch.len() <= MAXIMUM_BRANCH_LENGTH, "{branch}");
    }

    #[test]
    fn a_title_in_another_script_still_names_a_branch_git_accepts() {
        let branch = generate(&GitService::new(), "修复登录问题");

        assert_eq!(branch, "task/12345678");
    }

    #[test]
    fn a_title_with_nothing_in_it_names_the_task_without_a_dangling_hyphen() {
        for title in ["", "   ", "!!!"] {
            assert_eq!(generate(&GitService::new(), title), "task/12345678");
        }
    }

    #[test]
    fn the_namespace_the_author_and_the_length_are_the_callers_to_choose() {
        let service = GitService::new()
            .with_branch_prefix(BranchName::parse("agent").unwrap())
            .with_author("Ada", "ada@example.test")
            .with_maximum_branch_length(20);

        let branch = generate(&service, "a title far longer than twenty");

        assert_eq!(branch, "agent/12345678-a-tit");
        assert_eq!(service.author_name, "Ada");
        assert_eq!(service.author_email, "ada@example.test");
    }

    #[test]
    fn a_cut_never_leaves_a_separator_at_the_end() {
        for length in 13..=30 {
            let branch = generate(
                &GitService::new().with_maximum_branch_length(length),
                "ab cd ef gh ij",
            );

            assert!(!branch.ends_with(TRAILING), "{length}: {branch}");
            assert!(
                branch.len() <= length.max("task/12345678".len()),
                "{branch}"
            );
        }
    }

    #[test]
    fn a_prefix_outside_ascii_is_never_cut_through_a_character() {
        let service = GitService::new()
            .with_branch_prefix(BranchName::parse("ünïcødé/ağaç").unwrap())
            .with_maximum_branch_length(3);

        assert_eq!(generate(&service, "anything"), "ünïcødé/ağaç/12345678");
    }

    #[test]
    fn a_length_shorter_than_the_task_identifier_keeps_the_identifier() {
        let branch = generate(&GitService::new().with_maximum_branch_length(0), "title");

        assert_eq!(branch, "task/12345678");
    }

    #[test]
    fn a_prefix_too_long_for_git_to_hold_a_name_under_is_refused() {
        let service =
            GitService::new().with_branch_prefix(BranchName::parse(&"a".repeat(250)).unwrap());

        assert!(service.generate_branch_name(task(), "title").is_err());
    }
}

#[cfg(all(test, unix))]
mod process_tests {
    use super::hardened::fixtures::TEARDOWN_BUDGET;
    use super::hardened::fixtures::alive;
    use super::hardened::fixtures::marker;
    use super::*;
    use nix::sys::signal::Signal;
    use nix::sys::signal::kill;
    use nix::unistd::Pid;
    use std::os::unix::fs::PermissionsExt;

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
        let mut command = GitService::connected(
            &RepositoryUrl::parse("https://github.com/fixture/repository").unwrap(),
            Some(&SecretValue::new("fixture-credential")),
        );
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
            assert!(matches!(result, Err(GitError::TimedOut)), "{result:?}");
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

    /// Once a command has been waited for, its group identifier is free for
    /// the system to reuse, so nothing may be signalled through it any more.
    #[tokio::test]
    async fn a_completed_command_leaves_its_group_alone_once_reaped() {
        let fixture = tempfile::tempdir().unwrap();
        std::fs::write(
            fixture.path().join("git"),
            "#!/bin/sh\n/bin/sleep 60 >/dev/null 2>&1 &\necho $! > \"$FIXTURE/background\"\n",
        )
        .unwrap();
        std::fs::set_permissions(
            fixture.path().join("git"),
            std::fs::Permissions::from_mode(0o700),
        )
        .unwrap();
        let mut command = GitService::hardened();
        command
            .env("PATH", fixture.path())
            .env("FIXTURE", fixture.path());

        GitService::finish(&mut command).await.unwrap();
        let background = marker(fixture.path(), "background").await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        let survived = alive(background);
        let _ = kill(Pid::from_raw(background as i32), Signal::SIGKILL);

        assert!(survived, "a reaped command's group was signalled");
    }
}
