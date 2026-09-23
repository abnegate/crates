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

use crate::branch_name::BranchName;
use crate::commit_sha::CommitSha;
use crate::git::CONFIG_LISTING;
use crate::git::GITLINK_MODE;
use crate::git::IGNORE_SUBMODULES;
use crate::git::WorktreeEntry;
use crate::git::harden;
use crate::git::refused;
pub use crate::worktree::unfinished::Unfinished;
use std::io::Read;
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::path::PathBuf;
use std::process::Child;
use std::process::Command;
use std::process::Stdio;
use std::sync::mpsc::RecvTimeoutError;
use std::sync::mpsc::sync_channel;
use std::time::Duration;

/// The suffix the directory holding a repository's worktrees carries.
const AREA_SUFFIX: &str = "-worktrees";

/// What a path segment that may not carry a separator falls back to.
const REPLACEMENT: &str = "_";

/// The file a repository's own ignore rules live in.
const IGNORE_FILE: &str = ".gitignore";

/// Most standard output a local git command may produce before it is torn
/// down: a run can fill a worktree with untracked files whose names alone run
/// to gigabytes, and the host reads these listings whole. Output past this is
/// refused rather than held, which every caller here treats as a worktree that
/// cannot be confirmed empty and so is kept rather than removed.
const MAXIMUM_OUTPUT_BYTES: usize = 16 << 20;

/// Longest a local git command may run before it is torn down. These commands
/// reach no network, so this only bounds a command wedged on its own output.
const OUTPUT_TIMEOUT: Duration = Duration::from_secs(120);

/// How much of a command's output is read at a time.
const CHUNK: usize = 8192;

/// The status that decides whether a worktree holds tracked changes: no
/// untracked files, which are listed separately, and no descent into a nested
/// repository, whose presence is read from the index instead.
const TRACKED_STATUS: [&str; 5] = [
    "status",
    "--porcelain",
    "-z",
    "--untracked-files=no",
    IGNORE_SUBMODULES,
];

/// A git invocation with the pins and environment the hardened
/// [`crate::git::GitService`] commands run with, that additionally may use no
/// transport at all, so nothing here can reach the network -- not even a lazy
/// fetch of an object the clone lacks -- and needs no timeout.
fn local(repository: &Path) -> Command {
    let mut command = Command::new("git");
    harden(&mut command);
    command
        .env("GIT_ALLOW_PROTOCOL", "")
        .current_dir(repository)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

/// Refuse a repository whose own configuration holds anything beyond what git
/// writes for a clone, a worktree and a tracking branch: the configuration is
/// the base clone's, which every run of the repository can write through its
/// own git commands.
fn verify(repository: &Path) -> std::io::Result<()> {
    let listing = run(
        local(repository).args(CONFIG_LISTING),
        "read the repository's configuration",
    )?;
    match refused(&listing) {
        Some(key) => Err(std::io::Error::other(format!(
            "refusing a repository whose configuration sets {key:?}"
        ))),
        None => Ok(()),
    }
}

/// Run a local git command, reading at most [`MAXIMUM_OUTPUT_BYTES`] `+ 1` of
/// its standard output under [`OUTPUT_TIMEOUT`]. Output that runs past the cap,
/// or a command that outlives the timeout, tears the process group down and is
/// reported as a failure, so no run can make the host hold an unbounded
/// listing or wait on a wedged command.
fn run(command: &mut Command, what: &str) -> std::io::Result<Vec<u8>> {
    command.stdout(Stdio::piped()).stderr(Stdio::null());
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command.spawn()?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| std::io::Error::other("git command has no standard output"))?;
    let (sender, receiver) = sync_channel::<Vec<u8>>(1);
    std::thread::spawn(move || {
        let mut collected = Vec::new();
        let mut chunk = [0u8; CHUNK];
        loop {
            match stdout.read(&mut chunk) {
                Ok(0) => break,
                Ok(read) => {
                    let room = (MAXIMUM_OUTPUT_BYTES + 1).saturating_sub(collected.len());
                    collected.extend_from_slice(&chunk[..read.min(room)]);
                    if collected.len() > MAXIMUM_OUTPUT_BYTES {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        let _ = sender.send(collected);
    });
    match receiver.recv_timeout(OUTPUT_TIMEOUT) {
        Ok(collected) if collected.len() > MAXIMUM_OUTPUT_BYTES => {
            terminate(&mut child);
            Err(std::io::Error::other(format!(
                "git produced too much output to {what}"
            )))
        }
        Ok(collected) => {
            let status = child.wait()?;
            match status.success() {
                true => Ok(collected),
                false => Err(std::io::Error::other(format!("git could not {what}"))),
            }
        }
        Err(RecvTimeoutError::Timeout) => {
            terminate(&mut child);
            Err(std::io::Error::other(format!(
                "git took too long to {what}"
            )))
        }
        Err(RecvTimeoutError::Disconnected) => {
            terminate(&mut child);
            Err(std::io::Error::other(format!("git could not {what}")))
        }
    }
}

/// Kill a local git command and, where the platform has process groups, every
/// helper it started with it.
fn terminate(child: &mut Child) {
    #[cfg(unix)]
    {
        let _ = nix::sys::signal::killpg(
            nix::unistd::Pid::from_raw(child.id() as i32),
            nix::sys::signal::Signal::SIGKILL,
        );
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// Where a repository's worktrees live, and where one of them lives.
///
/// `{workspace}/{repository name}-worktrees/{identifier}`, with everything that
/// would open a second path segment replaced, so neither name can reach out of
/// the area the workspace set aside for it. An empty identifier names no
/// worktree.
pub fn path(workspace: &Path, repository_name: &str, identifier: &str) -> Option<PathBuf> {
    if identifier.is_empty() {
        return None;
    }
    let short_name = repository_name
        .split('/')
        .next_back()
        .unwrap_or(repository_name)
        .replace(['/', '\\', '\0'], REPLACEMENT);
    let identifier = identifier.replace(['/', '\\', '.', '\0'], REPLACEMENT);
    Some(
        workspace
            .join(format!("{short_name}{AREA_SUFFIX}"))
            .join(identifier),
    )
}

/// Add a detached worktree of `repository` at `path`, checked out at `start`.
/// Detached, because the run's own branch is made afterwards by the same
/// step that makes it in a clone, and a worktree that started on a named
/// branch would pin that branch to itself.
pub fn add(repository: &Path, path: &Path, start: &str) -> std::io::Result<()> {
    verify(repository)?;
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
/// pushed, if any.
///
/// Changes are read three ways, so no setting a run wrote into the shared
/// configuration and no mark it set in the index can hide one: tracked
/// changes from the status; files git does not track from the directory
/// itself, excluding only what the repository's own `.gitignore` files cover
/// -- those rules are what the repository declares disposable, and a rule a
/// run adds is itself a change the check sees, a new `.gitignore` included
/// even when it ignores itself -- and not what an excludes file or
/// `info/exclude` does; and every entry marked assume-unchanged or
/// skip-worktree, whose changes a status never reports; and any gitlink the
/// index records, which the status is told not to descend into -- descending
/// runs a child git under the nested repository's own configuration -- and
/// whose presence is counted as work rather than followed.
pub fn unfinished(path: &Path, known: &[&str]) -> std::io::Result<Unfinished> {
    verify(path)?;
    let tracked = run(
        local(path).args(TRACKED_STATUS),
        "read the worktree's status",
    )?;
    let staged = run(
        local(path).args(["ls-files", "-s", "-z"]),
        "read the worktree's staged entries",
    )?;
    let untracked = run(
        local(path).args([
            "ls-files",
            "--others",
            "--exclude-per-directory=.gitignore",
            "-z",
        ]),
        "list the worktree's untracked files",
    )?;
    let everything = run(
        local(path).args(["ls-files", "--others", "-z"]),
        "list every file the worktree does not track",
    )?;
    let marked = run(
        local(path).args(["ls-files", "-v", "-z"]),
        "read the worktree's index",
    )?;
    let head = run(
        local(path).args(["rev-parse", "--verify", "HEAD^{commit}"]),
        "read the worktree's head",
    )?;
    let head = String::from_utf8_lossy(&head).trim().to_string();
    Ok(Unfinished {
        uncommitted: !tracked.is_empty()
            || !untracked.is_empty()
            || adds_ignore_rules(&everything)
            || hides_changes(&marked)
            || holds_gitlink(&staged),
        unpublished: !known.iter().any(|commit| *commit == head),
    })
}

/// Whether an `ls-files -s` listing records a gitlink: a nested repository the
/// index points at, which the status is told not to enter and which a run can
/// leave behind. Its presence is treated as work rather than followed into.
fn holds_gitlink(listing: &[u8]) -> bool {
    listing
        .split(|byte| *byte == 0)
        .any(|entry| entry.starts_with(GITLINK_MODE.as_bytes()))
}

/// Whether a listing of untracked files holds a `.gitignore`, anywhere: a new
/// one is itself a change, and one that ignores itself hides every other file
/// beneath it from the listing that honours it.
fn adds_ignore_rules(listing: &[u8]) -> bool {
    listing
        .split(|byte| *byte == 0)
        .any(|entry| entry.rsplit(|byte| *byte == b'/').next() == Some(IGNORE_FILE.as_bytes()))
}

/// Whether an `ls-files -v` listing marks any entry assume-unchanged, which it
/// tags in lowercase, or skip-worktree, which it tags `S`.
fn hides_changes(listing: &[u8]) -> bool {
    listing
        .split(|byte| *byte == 0)
        .filter_map(|entry| entry.first())
        .any(|tag| tag.is_ascii_lowercase() || *tag == b'S')
}

/// The branch the worktree's HEAD names, or `None` when it is detached or
/// cannot be read. Where that branch is itself a symbolic ref, it is the
/// branch, not the ref the link names.
pub fn branch(path: &Path) -> Option<BranchName> {
    verify(path).ok()?;
    run(
        local(path).args(["symbolic-ref", "--quiet", "--short", "--no-recurse", "HEAD"]),
        "read the worktree's branch",
    )
    .ok()
    .and_then(|output| BranchName::parse(String::from_utf8_lossy(&output).trim()).ok())
}

/// Remove a worktree whether or not it is clean — the caller has decided,
/// on [`unfinished`], that nothing in it is lost — and prune the repository's
/// record of it. A branch the worktree was on is deleted with it, unless
/// another worktree has it checked out: its commits are on the remote, that
/// is what clean means, and a local ref left behind would refuse the next run
/// of the same task its own branch.
pub fn remove(repository: &Path, path: &Path) -> std::io::Result<()> {
    verify(repository)?;
    let on = branch(path);
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
        && let Err(error) = delete_branch(repository, &name)
    {
        tracing::warn!(branch = %name, %error, "Removed a worktree but could not delete its branch");
    }
    Ok(())
}

/// Delete `branch` by its ref alone, and only while it still names the
/// commit read for it and no worktree has it checked out; a branch that is a
/// symbolic ref is deleted as the link, never the ref it names. `branch -D`
/// would also drop the branch's section from the configuration, and git does
/// that by renaming a rewritten file over it, through a link wherever
/// `.git/config` is one, even when there is no section to drop.
fn delete_branch(repository: &Path, branch: &BranchName) -> std::io::Result<()> {
    let reference = branch.reference();
    let listing = run(
        local(repository).args(["worktree", "list", "--porcelain", "-z"]),
        "list the repository's worktrees",
    )?;
    if WorktreeEntry::parse(&listing)
        .iter()
        .any(|entry| entry.branch.as_deref() == Some(reference.as_str()))
    {
        return Err(std::io::Error::other(
            "another worktree has the branch checked out",
        ));
    }
    let commit = run(
        local(repository).args(["rev-parse", "--verify", &reference]),
        "read the branch's commit",
    )?;
    let commit =
        CommitSha::parse(&String::from_utf8_lossy(&commit)).map_err(std::io::Error::other)?;
    run(
        local(repository).args([
            "update-ref",
            "--no-deref",
            "-d",
            "--",
            &reference,
            commit.as_str(),
        ]),
        "delete the branch",
    )
    .map(drop)
}

/// The repository a worktree belongs to: the directory holding the `.git`
/// its `.git` file points into.
pub fn repository_of(path: &Path) -> std::io::Result<PathBuf> {
    verify(path)?;
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
    #[cfg(unix)]
    use std::os::unix::fs::MetadataExt;
    use std::path::Path;
    #[cfg(unix)]
    use std::path::PathBuf;
    use std::process::Command;

    /// Git in a fixture with a fixed identity and no host configuration.
    fn command(path: &Path, arguments: &[&str]) -> Command {
        let mut command = Command::new("git");
        command
            .env("GIT_AUTHOR_NAME", "Fixture")
            .env("GIT_AUTHOR_EMAIL", "fixture@example.test")
            .env("GIT_COMMITTER_NAME", "Fixture")
            .env("GIT_COMMITTER_EMAIL", "fixture@example.test")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_TERMINAL_PROMPT", "0")
            .args(arguments)
            .current_dir(path);
        command
    }

    /// Run git in a fixture with a fixed identity, panicking on failure.
    pub fn git(path: &Path, arguments: &[&str]) -> String {
        let output = command(path, arguments).output().expect("git runs");
        assert!(
            output.status.success(),
            "git {arguments:?} in {}: {}",
            path.display(),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    /// Run git in a fixture as [`git`] does, and say whether it succeeded.
    pub fn attempt(path: &Path, arguments: &[&str]) -> bool {
        command(path, arguments)
            .output()
            .expect("git runs")
            .status
            .success()
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

    /// A repository whose `.git/config` was replaced by a link to a copy of
    /// it, with what the copy held and which file it was when linked.
    #[cfg(unix)]
    pub struct LinkedConfig {
        copy: PathBuf,
        content: Vec<u8>,
        inode: u64,
    }

    #[cfg(unix)]
    impl LinkedConfig {
        /// Copy `repository`'s configuration to `copy` and link it there.
        pub fn new(repository: &Path, copy: &Path) -> Self {
            let config = repository.join(".git").join("config");
            std::fs::copy(&config, copy).unwrap();
            std::fs::remove_file(&config).unwrap();
            std::os::unix::fs::symlink(copy, &config).unwrap();
            Self {
                copy: copy.to_path_buf(),
                content: std::fs::read(copy).unwrap(),
                inode: std::fs::metadata(copy).unwrap().ino(),
            }
        }

        /// Panic unless the copy is still the same file holding the same
        /// bytes, with no lock file git left beside it.
        pub fn assert_untouched(&self) {
            assert_eq!(
                String::from_utf8_lossy(&std::fs::read(&self.copy).unwrap()),
                String::from_utf8_lossy(&self.content),
                "the linked file was written"
            );
            assert_eq!(
                std::fs::metadata(&self.copy).unwrap().ino(),
                self.inode,
                "the linked file was rewritten, if with the same content"
            );
            let mut lock = self.copy.clone().into_os_string();
            lock.push(".lock");
            assert!(
                std::fs::symlink_metadata(&lock).is_err(),
                "a lock file was left beside the linked file"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::clone;
    use super::fixtures::git;
    use super::fixtures::remote;
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
        assert_eq!(branch(&path), None, "detached, so no branch is pinned");
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
        assert_eq!(
            branch(&path).as_ref().map(BranchName::as_str),
            Some("task/one")
        );
        assert_eq!(
            git(&repositories.remote, &["rev-parse", "task/one"]),
            pushed
        );
    }

    /// A run's git commands reach the shared configuration, and a setting
    /// there that hides untracked files -- `status.showUntrackedFiles`, or an
    /// excludes file that ignores everything -- refuses the check rather than
    /// hiding them from it. A file the repository's own committed
    /// `.gitignore` covers is not counted.
    #[test]
    fn a_clone_told_to_hide_untracked_files_is_refused_and_committed_ignores_hold() {
        let repositories = repositories();
        let path = repositories.worktrees.join("run");
        add(&repositories.base, &path, "origin/HEAD").unwrap();
        let start = git(&path, &["rev-parse", "HEAD"]);
        for (key, value) in [
            ("status.showUntrackedFiles", "no"),
            ("core.excludesFile", "/dev/null"),
        ] {
            git(&path, &["config", key, value]);
            assert!(unfinished(&path, &[&start]).is_err(), "{key}");
            git(&path, &["config", "--unset", key]);
        }
        std::fs::write(path.join("notes.txt"), "not yet added\n").unwrap();
        assert!(unfinished(&path, &[&start]).unwrap().uncommitted);
        std::fs::remove_file(path.join("notes.txt")).unwrap();
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
        assert_eq!(
            branch(&second).as_ref().map(BranchName::as_str),
            Some("task/one")
        );
    }

    /// Removing a worktree deletes its branch by the ref alone, so the base
    /// clone's configuration is never rewritten, through a link wherever
    /// `.git/config` is one.
    #[cfg(unix)]
    #[test]
    fn removing_a_worktree_writes_nothing_through_a_linked_configuration() {
        let repositories = repositories();
        let path = repositories.worktrees.join("run");
        add(&repositories.base, &path, "origin/HEAD").unwrap();
        git(&path, &["checkout", "-q", "-b", "task/one"]);
        let linked = fixtures::LinkedConfig::new(
            &repositories.base,
            &repositories.base.with_file_name("copy"),
        );

        remove(&repositories.base, &path).unwrap();

        assert!(!path.exists(), "the worktree was removed");
        assert_eq!(
            git(&repositories.base, &["branch", "--list", "task/one"]),
            "",
            "the branch went with the worktree"
        );
        linked.assert_untouched();
    }

    /// A branch another worktree has checked out stays where it is when a
    /// worktree that was also on it is removed: deleting it would leave that
    /// worktree on a branch with no commit.
    #[test]
    fn removing_a_worktree_keeps_a_branch_another_worktree_has_checked_out() {
        let repositories = repositories();
        let first = repositories.worktrees.join("first");
        add(&repositories.base, &first, "origin/HEAD").unwrap();
        git(&first, &["checkout", "-q", "-b", "task/one"]);
        let second = repositories.worktrees.join("second");
        git(
            &repositories.base,
            &[
                "worktree",
                "add",
                "-q",
                "--force",
                second.to_str().unwrap(),
                "task/one",
            ],
        );

        remove(&repositories.base, &first).unwrap();

        assert!(!first.exists(), "the worktree was removed");
        assert_eq!(
            branch(&second).as_ref().map(BranchName::as_str),
            Some("task/one")
        );
        assert_eq!(
            git(&second, &["rev-parse", "--verify", "refs/heads/task/one"]),
            git(&second, &["rev-parse", "HEAD"]),
            "the branch the other worktree is on was kept"
        );
    }

    /// A task branch that is a link to the branch another worktree has
    /// checked out is deleted as the link alone: `worktree list` names the
    /// branch the link resolves to, so the check before the delete cannot see
    /// that worktree, and a delete through the link would take its branch.
    #[test]
    fn deleting_a_branch_that_is_a_link_leaves_the_branch_it_names() {
        let repositories = repositories();
        let other = repositories.worktrees.join("other");
        add(&repositories.base, &other, "origin/HEAD").unwrap();
        git(&other, &["checkout", "-q", "-b", "task/other"]);
        git(&other, &["commit", "-q", "--allow-empty", "-m", "work"]);
        let held = git(&other, &["rev-parse", "HEAD"]);
        git(
            &repositories.base,
            &[
                "symbolic-ref",
                "refs/heads/task/one",
                "refs/heads/task/other",
            ],
        );

        delete_branch(&repositories.base, &BranchName::parse("task/one").unwrap()).unwrap();

        assert_eq!(
            git(
                &repositories.base,
                &[
                    "for-each-ref",
                    "--format=%(refname) %(objectname)",
                    "refs/heads/task/"
                ],
            ),
            format!("refs/heads/task/other {held}"),
            "the link was deleted and the branch it names was kept"
        );
        assert_eq!(git(&other, &["rev-parse", "HEAD"]), held);
        assert_eq!(
            branch(&other).as_ref().map(BranchName::as_str),
            Some("task/other")
        );
    }

    /// A worktree whose HEAD names a branch that is a link is on the link,
    /// not on the branch the link names: removing it deletes the link and
    /// leaves that branch, which nothing asked to delete.
    #[test]
    fn removing_a_worktree_on_a_link_deletes_the_link_and_not_the_branch_it_names() {
        let repositories = repositories();
        let kept = git(
            &repositories.base,
            &["commit-tree", "-p", "HEAD", "-m", "work", "HEAD^{tree}"],
        );
        git(
            &repositories.base,
            &["update-ref", "refs/heads/task/other", &kept],
        );
        git(
            &repositories.base,
            &[
                "symbolic-ref",
                "refs/heads/task/one",
                "refs/heads/task/other",
            ],
        );
        let path = repositories.worktrees.join("run");
        add(&repositories.base, &path, "origin/HEAD").unwrap();
        git(&path, &["symbolic-ref", "HEAD", "refs/heads/task/one"]);
        let on = branch(&path);

        remove(&repositories.base, &path).unwrap();

        assert_eq!(
            git(
                &repositories.base,
                &[
                    "for-each-ref",
                    "--format=%(refname) %(objectname)",
                    "refs/heads/task/"
                ],
            ),
            format!("refs/heads/task/other {kept}"),
            "the link was deleted and the branch it names was kept"
        );
        assert_eq!(on.as_ref().map(BranchName::as_str), Some("task/one"));
    }

    #[test]
    fn a_missing_worktree_is_an_error_and_not_a_panic() {
        let repositories = repositories();
        let missing = repositories.worktrees.join("missing");
        assert!(unfinished(&missing, &[]).is_err());
        assert!(remove(&repositories.base, &missing).is_err());
        assert!(repository_of(&missing).is_err());
        assert_eq!(branch(&missing), None);
    }

    #[test]
    fn a_worktree_lives_under_an_area_named_for_its_repository() {
        assert_eq!(
            path(Path::new("/work"), "owner/repository", "ABC-123"),
            Some(PathBuf::from("/work/repository-worktrees/ABC-123"))
        );
        assert_eq!(
            path(Path::new("/work"), "repository", "XYZ-1"),
            Some(PathBuf::from("/work/repository-worktrees/XYZ-1")),
            "a name with no owner is its own short name"
        );
        assert_eq!(
            path(Path::new("/work"), "org/team/repository", "ISSUE-1"),
            Some(PathBuf::from("/work/repository-worktrees/ISSUE-1")),
            "only the last segment names the area"
        );
        assert_eq!(
            path(Path::new("/work"), "org/my-cool_repository", "ID-1"),
            Some(PathBuf::from("/work/my-cool_repository-worktrees/ID-1")),
            "hyphens and underscores are part of a name"
        );
        assert_eq!(
            path(
                Path::new("/var/lib/workspaces"),
                "myorg/backend",
                "JIRA-4567"
            ),
            Some(PathBuf::from(
                "/var/lib/workspaces/backend-worktrees/JIRA-4567"
            ))
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
                Some(PathBuf::from("/work").join(expected)),
                "{repository_name:?} / {identifier:?}"
            );
        }
        assert_eq!(
            path(Path::new("/work"), "owner/repository", ""),
            None,
            "an empty identifier names no worktree"
        );
    }

    /// A file only `info/exclude` ignores is not one the repository declares
    /// disposable: a run can write that file.
    #[test]
    fn a_file_hidden_only_by_the_clone_s_own_exclude_file_still_counts() {
        let repositories = repositories();
        let path = repositories.worktrees.join("run");
        add(&repositories.base, &path, "origin/HEAD").unwrap();
        let start = git(&path, &["rev-parse", "HEAD"]);
        let exclude = git(&path, &["rev-parse", "--git-path", "info/exclude"]);
        let exclude = path.join(exclude);
        std::fs::create_dir_all(exclude.parent().unwrap()).unwrap();
        std::fs::write(&exclude, "notes.txt\n").unwrap();
        std::fs::write(path.join("notes.txt"), "not yet added\n").unwrap();

        assert!(unfinished(&path, &[&start]).unwrap().uncommitted);
    }

    /// An entry marked assume-unchanged or skip-worktree hides its changes
    /// from a status, so the mark itself counts.
    #[test]
    fn a_change_hidden_by_an_index_mark_still_counts() {
        for mark in ["--assume-unchanged", "--skip-worktree"] {
            let repositories = repositories();
            let path = repositories.worktrees.join("run");
            add(&repositories.base, &path, "origin/HEAD").unwrap();
            let start = git(&path, &["rev-parse", "HEAD"]);
            git(&path, &["update-index", mark, "README"]);
            std::fs::write(path.join("README"), "changed and hidden\n").unwrap();
            assert_eq!(
                git(&path, &["status", "--porcelain"]),
                "",
                "{mark} hides the change from a status"
            );

            assert!(unfinished(&path, &[&start]).unwrap().uncommitted, "{mark}");
        }
    }

    /// Stat information git trusts under `core.ignoreStat` hides an edit made
    /// after the entry was refreshed; the setting is pinned off.
    #[test]
    fn a_change_hidden_by_trusted_stat_information_still_counts() {
        let repositories = repositories();
        let path = repositories.worktrees.join("run");
        add(&repositories.base, &path, "origin/HEAD").unwrap();
        let start = git(&path, &["rev-parse", "HEAD"]);
        git(&repositories.base, &["config", "core.ignoreStat", "true"]);
        git(&path, &["update-index", "--really-refresh"]);
        std::fs::write(path.join("README"), "changed\n").unwrap();
        assert!(
            unfinished(&path, &[&start]).is_err(),
            "the setting itself is refused"
        );
        git(
            &repositories.base,
            &["config", "--unset", "core.ignoreStat"],
        );

        let held = unfinished(&path, &[&start]).unwrap();

        assert!(
            held.uncommitted,
            "the marks it left behind still count: {held:?}"
        );
    }

    /// A clone whose configuration names a program for git to run refuses
    /// every operation here before git runs anything in it.
    #[test]
    fn a_clone_whose_configuration_names_a_driver_is_refused_before_anything_runs() {
        let repositories = repositories();
        let markers = tempfile::tempdir().unwrap();
        let marker = markers.path().join("ran");
        std::fs::write(
            repositories.base.join(".gitattributes"),
            "* filter=planted\n",
        )
        .unwrap();
        git(&repositories.base, &["add", ".gitattributes"]);
        git(&repositories.base, &["commit", "-q", "-m", "attributes"]);
        git(
            &repositories.base,
            &[
                "config",
                "filter.planted.smudge",
                &format!("touch '{}'; cat", marker.display()),
            ],
        );
        let path = repositories.worktrees.join("run");

        let refusal = add(&repositories.base, &path, "HEAD")
            .unwrap_err()
            .to_string();

        assert!(refusal.contains("filter.planted.smudge"), "{refusal}");
        assert!(!marker.exists(), "the smudge filter ran");
        assert!(!path.exists());
        assert!(unfinished(&repositories.base, &[]).is_err());
        assert!(repository_of(&repositories.base).is_err());
        assert!(remove(&repositories.base, &path).is_err());
    }

    #[test]
    fn a_local_command_may_use_no_transport_and_fetch_nothing_lazily() {
        let command = local(Path::new("."));
        let environment: Vec<(String, Option<String>)> = command
            .get_envs()
            .map(|(key, value)| {
                (
                    key.to_string_lossy().into_owned(),
                    value.map(|value| value.to_string_lossy().into_owned()),
                )
            })
            .collect();

        for (key, value) in [
            ("GIT_ALLOW_PROTOCOL", ""),
            ("GIT_NO_LAZY_FETCH", "1"),
            ("GIT_CONFIG_GLOBAL", "/dev/null"),
            ("GIT_CONFIG_SYSTEM", "/dev/null"),
        ] {
            assert!(
                environment.contains(&(key.to_string(), Some(value.to_string()))),
                "{key}={value} in {environment:?}"
            );
        }
    }

    /// A `.gitignore` a run adds that ignores itself hides everything beneath
    /// it from a listing that honours `.gitignore` files, so a new one counts
    /// on its own.
    #[test]
    fn a_new_ignore_file_that_hides_itself_still_counts() {
        for directory in ["", "nested/"] {
            let repositories = repositories();
            let path = repositories.worktrees.join("run");
            add(&repositories.base, &path, "origin/HEAD").unwrap();
            let start = git(&path, &["rev-parse", "HEAD"]);
            std::fs::create_dir_all(path.join(directory)).unwrap();
            std::fs::write(path.join(format!("{directory}.gitignore")), "*\n").unwrap();
            std::fs::write(path.join(format!("{directory}work.txt")), "unsaved\n").unwrap();

            assert!(
                unfinished(&path, &[&start]).unwrap().uncommitted,
                "{directory:?}"
            );
        }
    }

    #[test]
    fn the_status_reading_tracked_changes_stays_out_of_a_nested_repository() {
        assert!(
            TRACKED_STATUS.contains(&IGNORE_SUBMODULES),
            "{TRACKED_STATUS:?}"
        );
        assert!(
            TRACKED_STATUS.contains(&"--untracked-files=no"),
            "{TRACKED_STATUS:?}"
        );
    }

    /// A nested repository a run leaves in a worktree is reported as work and
    /// never entered: git is told to ignore submodules' own state, and the
    /// gitlink the index records is what marks the worktree unfinished, whether
    /// the nested repository is only sitting there or has been recorded.
    #[test]
    fn a_nested_repository_is_reported_as_work_and_never_entered() {
        let repositories = repositories();
        let path = repositories.worktrees.join("run");
        add(&repositories.base, &path, "origin/HEAD").unwrap();
        let start = git(&path, &["rev-parse", "HEAD"]);

        let nested = path.join("nested");
        std::fs::create_dir(&nested).unwrap();
        git(&nested, &["init", "-q", "-b", "main"]);
        assert!(
            unfinished(&path, &[&start]).unwrap().uncommitted,
            "an untracked nested repository is work"
        );

        std::fs::write(nested.join("file"), "a\n").unwrap();
        git(&nested, &["add", "file"]);
        git(&nested, &["commit", "-q", "-m", "nested"]);
        git(&path, &["add", "nested"]);
        git(&path, &["commit", "-q", "-m", "record gitlink"]);
        let head = git(&path, &["rev-parse", "HEAD"]);
        std::fs::write(
            nested.join("file"),
            "changed inside the nested repository\n",
        )
        .unwrap();

        let held = unfinished(&path, &[&start, &head]).unwrap();
        assert!(
            held.uncommitted,
            "a recorded gitlink is read from the index, not by entering it: {held:?}"
        );
    }

    /// A command flooding its output is capped and torn down rather than read
    /// whole, so a run cannot make the host hold gigabytes of a listing.
    #[cfg(unix)]
    #[test]
    fn a_command_that_floods_its_output_is_capped_and_torn_down() {
        let started = std::time::Instant::now();

        let result = run(&mut Command::new("yes"), "flood");

        assert!(
            result.is_err(),
            "unbounded output must be refused, not held"
        );
        assert!(
            started.elapsed() < Duration::from_secs(60),
            "the flood must be capped, not read to the end"
        );
    }
}
