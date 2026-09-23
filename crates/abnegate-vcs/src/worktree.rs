//! Worktrees for concurrent runs: one base clone per repository, one detached
//! worktree per run, and a removal rule that never throws work away.
//!
//! A run works in a worktree of its own under a directory the host chose, the
//! worktree is cleaned up when it is unchanged, and one that still holds work
//! nobody has is refused rather than removed. "Work nobody has" is changes no
//! commit holds, read from git, and a HEAD that is not a commit the caller
//! knows to be safe — the one the run started on, or one that was pushed —
//! which the caller states. Refs are not consulted for that: every ref in a
//! shared clone is a run's to move, and a run could make its commits look
//! published without a push.
//!
//! Everything here is local to the disk and synchronous, so it can run inside
//! a `Drop` as well as under `spawn_blocking`; the one network step, fetching
//! the base clone, stays on [`crate::git::GitService`] with its timeout.

mod unfinished;

pub use crate::worktree::unfinished::Unfinished;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::process::Stdio;

/// The suffix the directory holding a repository's worktrees carries.
const AREA_SUFFIX: &str = "-worktrees";

/// What a path segment that may not carry a separator falls back to.
const REPLACEMENT: &str = "_";

/// A git invocation that reads nothing from the host's configuration, runs
/// no program the repository's configuration names, and never talks to the
/// network: the hardening [`crate::git::GitService`] applies, for the local
/// operations that need no timeout. The repository configuration is the base
/// clone's, which every run of the repository can write through its own git
/// commands, so a hook path or a file-system monitor found there is not
/// honoured.
fn local(repository: &Path) -> Command {
    let mut command = Command::new("git");
    command
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_NO_REPLACE_OBJECTS", "1")
        .env("GIT_GRAFT_FILE", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "core.fsmonitor=false",
        ])
        .current_dir(repository)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

fn run(command: &mut Command, what: &str) -> std::io::Result<Vec<u8>> {
    let output = command.output()?;
    if !output.status.success() {
        // Git's stderr can quote paths and refs a caller supplied; the
        // operation is named instead, and the caller knows the path.
        return Err(std::io::Error::other(format!("git could not {what}")));
    }
    Ok(output.stdout)
}

/// Where a repository's worktrees live, and where one of them lives.
///
/// `{workspace}/{repository name}-worktrees/{identifier}`, with everything that
/// would open a second path segment replaced, so neither name can reach out of
/// the area the workspace set aside for it.
pub fn path(workspace: &Path, repository_name: &str, identifier: &str) -> PathBuf {
    let short_name = repository_name
        .split('/')
        .next_back()
        .unwrap_or(repository_name)
        .replace(['/', '\\', '\0'], REPLACEMENT);
    let identifier = identifier.replace(['/', '\\', '.', '\0'], REPLACEMENT);
    workspace
        .join(format!("{short_name}{AREA_SUFFIX}"))
        .join(identifier)
}

/// Add a detached worktree of `repository` at `path`, checked out at `start`.
/// Detached, because the run's own branch is made afterwards by the same
/// step that makes it in a clone, and a worktree that started on a named
/// branch would pin that branch to itself.
pub fn add(repository: &Path, path: &Path, start: &str) -> std::io::Result<()> {
    run(
        local(repository)
            .args(["worktree", "add", "--detach", "--"])
            .arg(path)
            .arg(start),
        "add a worktree",
    )
    .map(drop)
}

/// Whether `path` is a worktree rather than a clone of its own: git marks
/// one with a `.git` file pointing at the repository, where a clone has a
/// `.git` directory.
pub fn is_worktree(path: &Path) -> bool {
    path.join(".git").is_file()
}

/// What this worktree holds that nothing else does. `known` are the commits
/// the caller can vouch for: the one the run started on and the one that was
/// pushed, if any. The status is read with untracked files listed explicitly
/// and no excludes file taken from the configuration, so neither a
/// `status.showUntrackedFiles` nor a `core.excludesFile` a run wrote into the
/// shared configuration can hide a file from the check. A file the
/// repository's own ignore rules cover is not counted: those rules are what
/// the repository declares disposable, and a rule a run adds to `.gitignore`
/// is itself a change the check sees.
pub fn unfinished(path: &Path, known: &[&str]) -> std::io::Result<Unfinished> {
    let status = run(
        local(path).args([
            "-c",
            "status.showUntrackedFiles=all",
            "-c",
            "core.excludesFile=/dev/null",
            "status",
            "--porcelain",
            "--untracked-files=all",
        ]),
        "read the worktree's status",
    )?;
    let head = run(
        local(path).args(["rev-parse", "--verify", "HEAD^{commit}"]),
        "read the worktree's head",
    )?;
    let head = String::from_utf8_lossy(&head).trim().to_string();
    Ok(Unfinished {
        uncommitted: !status.is_empty(),
        unpublished: !known.iter().any(|commit| *commit == head),
    })
}

/// The branch the worktree is on, or `None` when it is detached.
pub fn branch(path: &Path) -> std::io::Result<Option<String>> {
    let name = run(
        local(path).args(["symbolic-ref", "--quiet", "--short", "HEAD"]),
        "read the worktree's branch",
    )
    .ok()
    .map(|output| String::from_utf8_lossy(&output).trim().to_string())
    .filter(|name| !name.is_empty());
    Ok(name)
}

/// Remove a worktree whether or not it is clean — the caller has decided,
/// on [`unfinished`], that nothing in it is lost — and prune the repository's
/// record of it. A branch the worktree was on is deleted with it: its commits
/// are on the remote, that is what clean means, and a local ref left behind
/// would refuse the next run of the same task its own branch.
pub fn remove(repository: &Path, path: &Path) -> std::io::Result<()> {
    let on = branch(path).unwrap_or(None);
    run(
        local(repository)
            .args(["worktree", "remove", "--force", "--"])
            .arg(path),
        "remove the worktree",
    )?;
    let _ = run(
        local(repository).args(["worktree", "prune"]),
        "prune worktrees",
    );
    if let Some(name) = on
        && let Err(error) = run(
            local(repository).args(["branch", "-D", "--", &name]),
            "delete the branch",
        )
    {
        // The worktree is gone and nothing holds the branch now; the next run
        // that asks for the name finds it unheld and takes it over.
        tracing::warn!(branch = %name, %error, "Removed a worktree but could not delete its branch");
    }
    Ok(())
}

/// The repository a worktree belongs to: the directory holding the `.git`
/// its `.git` file points into.
pub fn repository_of(path: &Path) -> std::io::Result<PathBuf> {
    let common = run(
        local(path).args(["rev-parse", "--path-format=absolute", "--git-common-dir"]),
        "find the worktree's repository",
    )?;
    let common = PathBuf::from(String::from_utf8_lossy(&common).trim());
    common
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| std::io::Error::other("git named a repository with no parent"))
}

#[cfg(test)]
pub(crate) mod fixtures {
    use std::path::Path;
    use std::process::Command;

    /// Run git in a fixture with a fixed identity, panicking on failure.
    pub fn git(path: &Path, arguments: &[&str]) -> String {
        let output = Command::new("git")
            .env("GIT_AUTHOR_NAME", "Fixture")
            .env("GIT_AUTHOR_EMAIL", "fixture@example.test")
            .env("GIT_COMMITTER_NAME", "Fixture")
            .env("GIT_COMMITTER_EMAIL", "fixture@example.test")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_TERMINAL_PROMPT", "0")
            .args(arguments)
            .current_dir(path)
            .output()
            .expect("git runs");
        assert!(
            output.status.success(),
            "git {arguments:?} in {}: {}",
            path.display(),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    /// A repository with one commit on `main`, the shape a remote has.
    pub fn remote(path: &Path) {
        git(path, &["init", "-q", "-b", "main"]);
        std::fs::write(path.join("README"), "fixture\n").unwrap();
        git(path, &["add", "README"]);
        git(path, &["commit", "-q", "-m", "fixture"]);
    }

    /// A clone of `remote` at `path`, as the base clone of a repository is.
    pub fn clone(remote: &Path, path: &Path) {
        git(
            remote.parent().unwrap(),
            &[
                "clone",
                "-q",
                remote.to_str().unwrap(),
                path.to_str().unwrap(),
            ],
        );
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::{clone, git, remote};
    use super::*;

    struct Repositories {
        _root: tempfile::TempDir,
        remote: PathBuf,
        base: PathBuf,
        worktrees: PathBuf,
    }

    fn repositories() -> Repositories {
        let root = tempfile::tempdir().unwrap();
        let remote_path = root.path().join("remote");
        std::fs::create_dir(&remote_path).unwrap();
        remote(&remote_path);
        let base = root.path().join("base");
        clone(&remote_path, &base);
        let worktrees = root.path().join("worktrees");
        std::fs::create_dir(&worktrees).unwrap();
        Repositories {
            _root: root,
            remote: remote_path,
            base,
            worktrees,
        }
    }

    #[test]
    fn a_worktree_starts_detached_at_the_remote_head_and_clean() {
        let repositories = repositories();
        let path = repositories.worktrees.join("run");
        add(&repositories.base, &path, "origin/HEAD").unwrap();
        assert!(is_worktree(&path), "a worktree is marked by a .git file");
        assert!(!is_worktree(&repositories.base), "the base is a clone");
        assert_eq!(
            std::fs::read_to_string(path.join("README")).unwrap(),
            "fixture\n"
        );
        assert_eq!(
            branch(&path).unwrap(),
            None,
            "detached, so no branch is pinned"
        );
        let start = git(&repositories.remote, &["rev-parse", "main"]);
        assert_eq!(unfinished(&path, &[&start]).unwrap(), Unfinished::default());
        assert!(!unfinished(&path, &[&start]).unwrap().any());
        assert_eq!(
            unfinished(&path, &[]).unwrap(),
            Unfinished {
                uncommitted: false,
                unpublished: true
            },
            "a head nobody vouches for is unpublished"
        );
        // Git names the repository by its real path, which on macOS is not
        // the path the temporary directory was handed out under.
        assert_eq!(
            repository_of(&path).unwrap(),
            repositories.base.canonicalize().unwrap()
        );
    }

    #[test]
    fn work_nobody_else_has_is_seen_at_each_stage_and_cleared_by_publication() {
        let repositories = repositories();
        let path = repositories.worktrees.join("run");
        add(&repositories.base, &path, "origin/HEAD").unwrap();
        let start = git(&path, &["rev-parse", "HEAD"]);
        git(&path, &["checkout", "-q", "-b", "task/one"]);
        std::fs::write(path.join("work.txt"), "in progress\n").unwrap();
        assert_eq!(
            unfinished(&path, &[&start]).unwrap(),
            Unfinished {
                uncommitted: true,
                unpublished: false
            }
        );
        git(&path, &["add", "work.txt"]);
        git(&path, &["commit", "-q", "-m", "work"]);
        assert_eq!(
            unfinished(&path, &[&start]).unwrap(),
            Unfinished {
                uncommitted: false,
                unpublished: true
            },
            "a commit nothing pushed is unpublished"
        );
        // A run can write any ref in the shared clone; a remote-tracking ref
        // planted without a push proves nothing.
        git(
            &path,
            &["update-ref", "refs/remotes/origin/task/one", "HEAD"],
        );
        assert!(
            unfinished(&path, &[&start]).unwrap().unpublished,
            "a planted remote-tracking ref is not a publication"
        );
        // Publication as the service performs it: a push, and the service
        // vouching for what it pushed.
        git(&path, &["push", "-q", "origin", "HEAD:refs/heads/task/one"]);
        let pushed = git(&path, &["rev-parse", "HEAD"]);
        assert_eq!(
            unfinished(&path, &[&start, &pushed]).unwrap(),
            Unfinished::default()
        );
        assert_eq!(branch(&path).unwrap().as_deref(), Some("task/one"));
        assert_eq!(
            git(&repositories.remote, &["rev-parse", "task/one"]),
            pushed
        );
    }

    /// A run's git commands reach the shared configuration, and a setting
    /// there that hides untracked files — `status.showUntrackedFiles`, or an
    /// excludes file that ignores everything — must not hide them from the
    /// check. A file the repository's own `.gitignore` covers is not counted.
    #[test]
    fn an_untracked_file_counts_even_when_the_clone_is_told_to_hide_them() {
        let repositories = repositories();
        let path = repositories.worktrees.join("run");
        add(&repositories.base, &path, "origin/HEAD").unwrap();
        let start = git(&path, &["rev-parse", "HEAD"]);
        git(&path, &["config", "status.showUntrackedFiles", "no"]);
        let excludes = repositories.worktrees.join("hide-everything");
        std::fs::write(&excludes, "*\n").unwrap();
        git(
            &path,
            &["config", "core.excludesFile", excludes.to_str().unwrap()],
        );
        std::fs::write(path.join("notes.txt"), "not yet added\n").unwrap();
        assert!(
            unfinished(&path, &[&start]).unwrap().uncommitted,
            "the file is seen despite status.showUntrackedFiles=no and an excludes file"
        );
        std::fs::remove_file(path.join("notes.txt")).unwrap();
        git(&path, &["config", "--unset", "core.excludesFile"]);
        std::fs::write(path.join(".gitignore"), "*.log\n").unwrap();
        git(&path, &["add", ".gitignore"]);
        git(&path, &["commit", "-q", "-m", "ignore logs"]);
        std::fs::write(path.join("build.log"), "output\n").unwrap();
        let head = git(&path, &["rev-parse", "HEAD"]);
        assert!(
            !unfinished(&path, &[&start, &head]).unwrap().uncommitted,
            "a file the repository's own rules ignore is not work"
        );
    }

    #[test]
    fn removing_a_worktree_takes_its_branch_and_leaves_the_base_usable() {
        let repositories = repositories();
        let first = repositories.worktrees.join("first");
        add(&repositories.base, &first, "origin/HEAD").unwrap();
        git(&first, &["checkout", "-q", "-b", "task/one"]);
        std::fs::write(first.join("work.txt"), "x\n").unwrap();
        remove(&repositories.base, &first).unwrap();
        assert!(
            !first.exists(),
            "removed even with an uncommitted file: the caller decided"
        );
        let listed = git(&repositories.base, &["worktree", "list", "--porcelain"]);
        assert!(!listed.contains("first"), "{listed}");
        let branches = git(&repositories.base, &["branch", "--list", "task/one"]);
        assert_eq!(branches, "", "the branch went with the worktree");
        // The next run of the same task can take the branch name again.
        let second = repositories.worktrees.join("second");
        add(&repositories.base, &second, "origin/HEAD").unwrap();
        git(&second, &["checkout", "-q", "-b", "task/one"]);
        assert_eq!(branch(&second).unwrap().as_deref(), Some("task/one"));
    }

    #[test]
    fn a_missing_worktree_is_an_error_and_not_a_panic() {
        let repositories = repositories();
        let missing = repositories.worktrees.join("missing");
        assert!(unfinished(&missing, &[]).is_err());
        assert!(remove(&repositories.base, &missing).is_err());
        assert!(repository_of(&missing).is_err());
    }

    #[test]
    fn a_worktree_lives_under_an_area_named_for_its_repository() {
        assert_eq!(
            path(Path::new("/work"), "owner/repository", "ABC-123"),
            PathBuf::from("/work/repository-worktrees/ABC-123")
        );
        assert_eq!(
            path(Path::new("/work"), "repository", "XYZ-1"),
            PathBuf::from("/work/repository-worktrees/XYZ-1"),
            "a name with no owner is its own short name"
        );
        assert_eq!(
            path(Path::new("/work"), "org/team/repository", "ISSUE-1"),
            PathBuf::from("/work/repository-worktrees/ISSUE-1"),
            "only the last segment names the area"
        );
        assert_eq!(
            path(Path::new("/work"), "org/my-cool_repository", "ID-1"),
            PathBuf::from("/work/my-cool_repository-worktrees/ID-1"),
            "hyphens and underscores are part of a name"
        );
        assert_eq!(
            path(
                Path::new("/var/lib/workspaces"),
                "myorg/backend",
                "JIRA-4567"
            ),
            PathBuf::from("/var/lib/workspaces/backend-worktrees/JIRA-4567")
        );
    }

    #[test]
    fn nothing_in_either_name_can_open_a_second_path_segment() {
        for (repository_name, identifier, expected) in [
            (
                "owner/repository",
                "feat/issue",
                "repository-worktrees/feat_issue",
            ),
            (
                "owner/repository",
                "feat\\issue",
                "repository-worktrees/feat_issue",
            ),
            ("owner/repository", "v1.2.3", "repository-worktrees/v1_2_3"),
            (
                "owner/repository",
                "issue\0evil",
                "repository-worktrees/issue_evil",
            ),
            (
                "owner/repository",
                "a/b\\c.d\0e",
                "repository-worktrees/a_b_c_d_e",
            ),
            ("owner/repository", "...", "repository-worktrees/___"),
            ("owner/repository", "///", "repository-worktrees/___"),
            ("evil\0repository", "ID-1", "evil_repository-worktrees/ID-1"),
            ("owner\\repository", "ID", "owner_repository-worktrees/ID"),
            ("re\0po", "is\0sue", "re_po-worktrees/is_sue"),
            (
                "owner/repository",
                "ABC-123-DEF",
                "repository-worktrees/ABC-123-DEF",
            ),
            (
                "owner/repository",
                "my_issue_123",
                "repository-worktrees/my_issue_123",
            ),
            (
                "owner/repository",
                "issue-\u{00e9}",
                "repository-worktrees/issue-\u{00e9}",
            ),
            ("owner/repository/", "ID-1", "-worktrees/ID-1"),
            ("", "ID-1", "-worktrees/ID-1"),
        ] {
            assert_eq!(
                path(Path::new("/work"), repository_name, identifier),
                PathBuf::from("/work").join(expected),
                "{repository_name:?} / {identifier:?}"
            );
        }
        assert_eq!(
            path(Path::new("/work"), "owner/repository", ""),
            PathBuf::from("/work/repository-worktrees/"),
            "an empty identifier names the area itself"
        );
    }
}
