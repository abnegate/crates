use crate::git::DiffSummary;
use crate::git::GitError;
use crate::git::GitResult;
use crate::git::RemoteHead;
use crate::git::authentication::authenticate;
#[cfg(unix)]
use crate::git::group::Group;
use std::path::Path;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;
use url::Url;
use uuid::Uuid;

mod hardened;
mod managed;
mod worktrees;

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
