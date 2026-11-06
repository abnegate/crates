use crate::git::GitError;
use crate::git::GitResult;
#[cfg(unix)]
use nix::unistd::AccessFlags;
#[cfg(unix)]
use nix::unistd::access;
use std::ffi::OsStr;
use std::ffi::OsString;
#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::process::Stdio;

/// Settings pinned on every command line, where a repository's own
/// configuration cannot override them: nothing a run wrote into it may run a
/// hook, a file-system monitor, a credential helper, a signing program or a
/// maintenance job, follow a redirect, turn off certificate checks, route
/// through a proxy, recurse into submodules, push tags or options nobody
/// named, mark files so a status stops seeing them, have a checkout or
/// switch enter a nested repository to list its changes, or start a reflog
/// under `logs/`, where git follows a link standing in a file's place.
///
/// `http.sslCAInfo` and `http.sslCAPath` are deliberately absent: git hands an
/// empty value to curl verbatim and every HTTPS request then fails. A
/// repository that sets either is refused by [`refused`] instead.
pub(crate) const PINS: [&str; 38] = [
    "-c",
    "core.hooksPath=/dev/null",
    "-c",
    "core.fsmonitor=false",
    "-c",
    "credential.helper=",
    "-c",
    "http.followRedirects=false",
    "-c",
    "commit.gpgsign=false",
    "-c",
    "tag.gpgsign=false",
    "-c",
    "http.sslVerify=true",
    "-c",
    "http.proxy=",
    "-c",
    "core.ignoreStat=false",
    "-c",
    "submodule.recurse=false",
    "-c",
    "fetch.recurseSubmodules=false",
    "-c",
    "push.followTags=false",
    "-c",
    "push.gpgSign=false",
    "-c",
    "push.negotiate=false",
    "-c",
    "push.pushOption=",
    "-c",
    "gc.auto=0",
    "-c",
    "maintenance.auto=false",
    "-c",
    "diff.ignoreSubmodules=dirty",
    "-c",
    "core.logAllRefUpdates=false",
];

/// Environment every hardened command runs under, after the host's own has
/// been cleared: no system or global configuration, no replacement objects or
/// grafts, no prompt or askpass program, pathspecs taken literally, no lazy
/// fetch of missing objects, and HTTPS as the only transport.
const ENVIRONMENT: [(&str, &str); 11] = [
    ("GIT_CONFIG_NOSYSTEM", "1"),
    ("GIT_CONFIG_SYSTEM", "/dev/null"),
    ("GIT_CONFIG_GLOBAL", "/dev/null"),
    ("GIT_NO_REPLACE_OBJECTS", "1"),
    ("GIT_GRAFT_FILE", "/dev/null"),
    ("GIT_TERMINAL_PROMPT", "0"),
    ("GIT_ASKPASS", ""),
    ("GIT_ALLOW_PROTOCOL", "https"),
    ("GIT_LITERAL_PATHSPECS", "1"),
    ("GIT_NO_LAZY_FETCH", "1"),
    ("LC_ALL", "C"),
];

/// The repository configuration git itself writes for a clone, a worktree and
/// a tracking branch, and nothing else. Every other key -- a transport
/// rewrite, an HTTP override, an include, a filter, diff or merge driver, a
/// hook, a command -- is one a run's git commands could have written, and git
/// keeps adding keys that run programs, so a repository carrying any key not
/// listed here is refused rather than inspected key by key.
const PERMITTED_KEYS: [&str; 10] = [
    "core.repositoryformatversion",
    "core.filemode",
    "core.bare",
    "core.logallrefupdates",
    "core.ignorecase",
    "core.precomposeunicode",
    "core.symlinks",
    "extensions.objectformat",
    "extensions.refstorage",
    "extensions.relativeworktrees",
];

/// A remote's keys that name where it is and what to fetch from it.
const PERMITTED_REMOTE_KEYS: [&str; 2] = ["url", "fetch"];

/// A branch's keys that name what it tracks.
const PERMITTED_BRANCH_KEYS: [&str; 2] = ["remote", "merge"];

/// The section configuring a named remote.
const REMOTE_SECTION: &str = "remote.";

/// The section configuring a local branch.
const BRANCH_SECTION: &str = "branch.";

/// The directory a repository keeps its objects in. Its loose objects run to
/// many thousands of files, so the walk [`unlinked`] makes looks at what
/// stands directly in it and walks only [`OBJECTS_WALKED`] below it.
const OBJECTS: &str = "objects";

/// The directories under [`OBJECTS`] walked whole: `info` holds
/// `alternates`, naming the further stores git reads objects from, and the
/// commit graphs git writes; `pack` holds the few packs a clone keeps, whose
/// times git sets through a link standing in a pack's place when it
/// freshens one.
const OBJECTS_WALKED: [&str; 2] = ["info", "pack"];

/// The file under [`OBJECTS`] naming the further stores git reads objects
/// from.
const ALTERNATES: [&str; 2] = ["info", "alternates"];

/// The variables git keeps local to one repository, exactly as
/// `git rev-parse --local-env-vars` lists them: what git itself clears
/// before it works in another repository, `GIT_DIR`, `GIT_INDEX_FILE` and
/// `GIT_COMMON_DIR` among them. A command that keeps the caller's
/// environment drops these, so the repository it works in is the one its
/// directory holds, even when the caller runs inside a git hook.
pub(crate) const REPOSITORY_ENVIRONMENT: &[&str] = &[
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_CONFIG",
    "GIT_CONFIG_PARAMETERS",
    "GIT_CONFIG_COUNT",
    "GIT_OBJECT_DIRECTORY",
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_IMPLICIT_WORK_TREE",
    "GIT_GRAFT_FILE",
    "GIT_INDEX_FILE",
    "GIT_NO_REPLACE_OBJECTS",
    "GIT_REPLACE_REF_BASE",
    "GIT_PREFIX",
    "GIT_SHALLOW_FILE",
    "GIT_COMMON_DIR",
];

/// Prints the worktree's own git directory and the one every worktree of
/// its repository shares, a line each, as absolute paths. Git resolves every
/// link on the way to an absolute path it prints, so what stands under them
/// is walked here rather than asked for: a path git printed for a link
/// would already name what the link points at.
pub(crate) const LOCATING: [&str; 4] = [
    "rev-parse",
    "--path-format=absolute",
    "--git-dir",
    "--git-common-dir",
];

/// Lists the repository's own configuration keys, NUL-terminated.
pub(crate) const CONFIG_LISTING: [&str; 5] = ["config", "--local", "--name-only", "--list", "-z"];

/// Passed to every status and diff so neither descends into a nested
/// repository standing in the working tree: doing so starts a child git inside
/// it that reads that repository's own configuration, and a run only has to
/// leave a `.git` directory behind for a program named there to run as the
/// host. A mode-160000 entry is detected separately, so ignoring a nested
/// repository's own dirty state loses no signal the host acts on.
pub(crate) const IGNORE_SUBMODULES: &str = "--ignore-submodules=dirty";

/// The mode `git ls-files -s` prints for a gitlink: a nested repository
/// recorded in the index rather than a file whose content the host controls.
pub(crate) const GITLINK_MODE: &str = "160000 ";

/// Passed to every fetch so it writes no `FETCH_HEAD`: git opens that file
/// by name and follows a link standing in its place, and nothing here reads
/// it, since every fetch names the refs it writes on its own command line.
pub(crate) const NO_FETCH_HEAD: &str = "--no-write-fetch-head";

/// The flags every diff carries before its own: no external or textconv
/// driver a repository could name, and no descent into a nested repository.
pub(crate) const DIFF_PREFIX: [&str; 4] =
    ["diff", "--no-ext-diff", "--no-textconv", IGNORE_SUBMODULES];

/// The name of the program every hardened command runs.
const GIT: &str = "git";

/// The variable a command searches for the programs it runs.
const SEARCH: &str = "PATH";

/// A directory that does not exist, which a hardened command searches when
/// the host's `PATH` names no absolute one, and under which it names git when
/// no entry holds one: an empty search path would send `execvp`, git and
/// every helper git starts to the directory the command runs in.
const NOWHERE: &str = "/nonexistent";

/// A git command that reads nothing from the host's configuration and runs no
/// program the repository's configuration names, with the pins and
/// environment every hardened command runs with. The host's `PATH` is read
/// once, and the command searches only its [`absolute`] entries.
///
/// Git is named by its whole path, the [`executable`] on those entries. A
/// command given a `PATH` of its own and a program to find on it cannot be
/// started with `posix_spawn`: the host forks, and the child searches that
/// `PATH` before it executes. In a host with many threads that child can
/// crash before git ever runs, and the command fails as though git had. A
/// command named by the program's path is spawned whole.
pub(crate) fn hardened() -> Command {
    let search = searched();
    let program = executable(&search);
    pinned(&program, search)
}

/// [`hardened`], running `program` in git's place.
#[cfg(test)]
pub(crate) fn invoking(program: &Path) -> Command {
    pinned(program, searched())
}

/// The [`absolute`] entries of the host's `PATH`, read once.
fn searched() -> OsString {
    absolute(&std::env::var_os(SEARCH).unwrap_or_default())
}

/// `program` with the pins and environment every hardened command runs
/// with, searching `search` and nothing else.
fn pinned(program: &Path, search: OsString) -> Command {
    let mut command = Command::new(program);
    command
        .env_clear()
        .env(SEARCH, search)
        .envs(ENVIRONMENT)
        .args(PINS)
        .stdin(Stdio::null());
    command
}

/// The git a hardened command searching `search` runs: the first executable
/// `git` in an absolute entry, or, where none holds one, a path under
/// [`NOWHERE`], which fails to spawn with [`std::io::ErrorKind::NotFound`].
#[cfg(unix)]
fn executable(search: &OsStr) -> PathBuf {
    found(search).unwrap_or_else(unfound)
}

/// The git a hardened command runs, found on the `PATH` it is given by the
/// system call that starts it, which forks nothing.
#[cfg(not(unix))]
fn executable(_search: &OsStr) -> PathBuf {
    PathBuf::from(GIT)
}

/// The path a hardened command names git by when no entry holds one.
#[cfg(unix)]
fn unfound() -> PathBuf {
    Path::new(NOWHERE).join(GIT)
}

/// The first executable `git` in an absolute entry of `path`, which is the
/// one `execvp` would run on the [`absolute`] entries of `path`.
#[cfg(unix)]
fn found(path: &OsStr) -> Option<PathBuf> {
    std::env::split_paths(path)
        .filter(|entry| entry.is_absolute())
        .map(|entry| entry.join(GIT))
        .find(|candidate| runnable(candidate))
}

/// Whether `candidate` is a file this process may execute, by the check
/// `execvp` makes.
#[cfg(unix)]
fn runnable(candidate: &Path) -> bool {
    candidate.is_file() && access(candidate, AccessFlags::X_OK).is_ok()
}

/// The absolute entries of a `PATH`, or [`NOWHERE`] when it has none. A
/// relative or empty entry is resolved against the directory a command runs
/// in, which for a hardened command is a repository a run can write, so a
/// `git` there would run instead of git.
fn absolute(path: &OsStr) -> OsString {
    std::env::join_paths(std::env::split_paths(path).filter(|entry| entry.is_absolute()))
        .ok()
        .filter(|joined| !joined.is_empty())
        .unwrap_or_else(|| OsString::from(NOWHERE))
}

/// Refuse a repository with a symbolic link anywhere under the worktree's
/// own git directory or the one every worktree of its repository shares,
/// given the directories [`LOCATING`] printed, with [`GitError::LinkedPath`]:
/// git writes its refs, ref tables, reflogs, index, worktree records, commit
/// message and configuration by name, and follows a link standing at any of
/// them or at a directory above one. No link is followed, so each is seen
/// where it stands; [`OBJECTS`] is looked at only as far as it says. Inside
/// the directories its loose objects fan out into, only the directories
/// themselves are looked at: git writes a loose object to a file of its
/// own and renames it into place, but freshens one already there by
/// setting its times, through a link standing in its place, so a link there
/// can have git touch the times, and never the content, of the file it
/// points at. What vanishes while the walk runs is fine. What cannot be
/// read, and a listing that is not one absolute directory for each, are
/// refused: what stands there cannot be known.
pub(crate) fn unlinked(located: &[u8]) -> GitResult<()> {
    let (own, shared) = directories(located)?;
    if own != shared {
        walk(&own)?;
    }
    walk(&shared)
}

/// Refuse a repository whose shared git directory, given the directories
/// [`LOCATING`] printed, holds anything at `objects/info/alternates`, with
/// [`GitError::AlternateObjects`]: git reads objects from every store that
/// file names, whatever the host can read, and a push uploads whatever the
/// pushed commit reaches from any of them. No clone this crate makes or
/// manages has one: each is made through git's transport, which sends
/// objects and never the source's files, a managed clone of a local path
/// included, since it is made with `--no-local`; git would otherwise copy
/// the alternates of a source made with `--shared` or `--reference` along
/// with its objects. A run's own commands can still write one. A worktree's
/// own git directory holds no objects, so only the shared one is looked at.
/// What cannot be looked at is refused as [`unlinked`] refuses it.
pub(crate) fn unborrowed(located: &[u8]) -> GitResult<()> {
    let (_, shared) = directories(located)?;
    let alternates = ALTERNATES
        .iter()
        .fold(shared.join(OBJECTS), |path, name| path.join(name));
    match present(std::fs::symlink_metadata(&alternates))? {
        Some(_) => Err(GitError::AlternateObjects),
        None => Ok(()),
    }
}

/// The worktree's own git directory and the one every worktree of its
/// repository shares, as [`LOCATING`] printed them. A listing that is not
/// one absolute directory for each is refused: what it names cannot be
/// known.
pub(crate) fn directories(located: &[u8]) -> GitResult<(PathBuf, PathBuf)> {
    let unlocated = || GitError::CommandFailed("Cannot locate the repository's files".to_string());
    let directories: Vec<PathBuf> = located
        .strip_suffix(b"\n")
        .unwrap_or(located)
        .split(|byte| *byte == b'\n')
        .map(|line| native(line).filter(|directory| directory.is_absolute()))
        .collect::<Option<_>>()
        .ok_or_else(unlocated)?;
    let [own, shared] = <[PathBuf; 2]>::try_from(directories).map_err(|_| unlocated())?;
    Ok((own, shared))
}

/// Refuse the first symbolic link at or below `root`, never following one.
fn walk(root: &Path) -> GitResult<()> {
    let objects = root.join(OBJECTS);
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let Some(entries) = present(std::fs::read_dir(&directory))? else {
            continue;
        };
        let whole = directory != objects;
        for entry in entries {
            let Some(entry) = present(entry)? else {
                continue;
            };
            let Some(kind) = present(entry.file_type())? else {
                continue;
            };
            if kind.is_symlink() {
                return Err(GitError::LinkedPath);
            }
            if kind.is_dir()
                && (whole
                    || OBJECTS_WALKED
                        .iter()
                        .any(|walked| entry.file_name() == *walked))
            {
                pending.push(entry.path());
            }
        }
    }
    Ok(())
}

/// What a look at the git directory found: nothing when what it looked at
/// vanished meanwhile, or became a file, as a ref directory can between the
/// look at what it is and the look inside it when a concurrent command
/// deletes the refs in it and writes a ref of its name; and a refusal when
/// it could not look.
fn present<T>(looked: std::io::Result<T>) -> GitResult<Option<T>> {
    match looked {
        Ok(found) => Ok(Some(found)),
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
            ) =>
        {
            Ok(None)
        }
        Err(_) => Err(GitError::CommandFailed(
            "Cannot read the repository's git directory".to_string(),
        )),
    }
}

/// A path or ref name git printed, byte for byte, so a name that is not
/// UTF-8 still names what git reads.
#[cfg(unix)]
pub(crate) fn native(bytes: &[u8]) -> Option<PathBuf> {
    Some(PathBuf::from(OsStr::from_bytes(bytes)))
}

/// A path or ref name git printed, as [`utf8`] reads it: git keeps names in
/// UTF-8 wherever the platform's own are not bytes.
#[cfg(not(unix))]
pub(crate) fn native(bytes: &[u8]) -> Option<PathBuf> {
    utf8(bytes)
}

/// A name git printed, when it is UTF-8. One that is not names nothing a
/// platform keeping names in UTF-8 can look up, and a lossy conversion
/// would name something else: a ref that is not there reads as no link, and
/// a path that is not there as nothing standing at it.
#[cfg(any(test, not(unix)))]
fn utf8(bytes: &[u8]) -> Option<PathBuf> {
    std::str::from_utf8(bytes).ok().map(PathBuf::from)
}

/// The first key of a [`CONFIG_LISTING`] a hardened command must not run
/// under, if there is one.
pub(crate) fn refused(listing: &[u8]) -> Option<String> {
    listing
        .split(|byte| *byte == 0)
        .filter(|key| !key.is_empty())
        .map(|key| String::from_utf8_lossy(key).to_ascii_lowercase())
        .find(|key| !permitted(key))
}

fn permitted(key: &str) -> bool {
    PERMITTED_KEYS.contains(&key)
        || scoped(key, REMOTE_SECTION, &PERMITTED_REMOTE_KEYS)
            .is_some_and(|name| !name.contains([':', '/']))
        || scoped(key, BRANCH_SECTION, &PERMITTED_BRANCH_KEYS).is_some()
}

/// The subsection of a `{section}{name}.{key}` entry whose key is one of
/// `keys`.
fn scoped<'a>(key: &'a str, section: &str, keys: &[&str]) -> Option<&'a str> {
    let (name, variable) = key.strip_prefix(section)?.rsplit_once('.')?;
    (!name.is_empty() && keys.contains(&variable)).then_some(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listing(keys: &[&str]) -> Vec<u8> {
        keys.iter().flat_map(|key| key.bytes().chain([0])).collect()
    }

    #[test]
    fn a_clone_s_own_configuration_is_accepted() {
        assert_eq!(
            refused(&listing(&[
                "core.repositoryformatversion",
                "core.filemode",
                "core.bare",
                "core.logallrefupdates",
                "core.ignorecase",
                "core.precomposeunicode",
                "core.symlinks",
                "remote.origin.url",
                "remote.origin.fetch",
                "remote.origin.fetch",
                "branch.main.remote",
                "branch.main.merge",
                "branch.task/one.merge",
                "extensions.objectformat",
                "extensions.refStorage",
                "extensions.relativeWorktrees",
            ])),
            None
        );
    }

    #[test]
    fn every_other_key_is_refused() {
        for key in [
            "hook.planted.command",
            "hook.planted.event",
            "url.https://elsewhere.test/.insteadof",
            "http.proxy",
            "http.https://github.com/.sslcainfo",
            "include.path",
            "includeIf.gitdir:/x.path",
            "filter.lfs.smudge",
            "diff.external",
            "diff.driver.textconv",
            "merge.driver.driver",
            "core.sshCommand",
            "core.alternateRefsCommand",
            "core.worktree",
            "core.excludesFile",
            "core.attributesFile",
            "core.autocrlf",
            "author.name",
            "user.email",
            "push.followTags",
            "fetch.bundleURI",
            "transfer.bundleURI",
            "extensions.worktreeConfig",
            "extensions.partialClone",
            "remote.origin.pushurl",
            "remote.origin.uploadpack",
            "remote.https://github.com/owner/repository.git.url",
            "remote.file:///tmp/x.fetch",
            "branch.main.pushRemote",
            "status.showUntrackedFiles",
        ] {
            assert_eq!(
                refused(&listing(&["core.bare", key])),
                Some(key.to_ascii_lowercase()),
                "{key}"
            );
        }
    }

    #[test]
    fn every_diff_carries_the_flag_that_keeps_it_out_of_a_nested_repository() {
        assert_eq!(IGNORE_SUBMODULES, "--ignore-submodules=dirty");
        assert_eq!(DIFF_PREFIX[0], "diff");
        assert!(
            DIFF_PREFIX.contains(&IGNORE_SUBMODULES),
            "every diff is built from this prefix, so every diff carries it: {DIFF_PREFIX:?}"
        );
        assert!(
            !DIFF_PREFIX
                .iter()
                .any(|flag| flag.contains("ignore-submodules=none")),
            "nothing must ask a diff to descend into a nested repository"
        );
    }

    #[test]
    fn a_checkout_or_switch_never_enters_a_nested_repository_to_list_its_changes() {
        let pinned = PINS
            .windows(2)
            .any(|pair| pair == ["-c", "diff.ignoreSubmodules=dirty"]);

        assert!(pinned, "{PINS:?}");
    }

    #[test]
    fn no_command_starts_a_reflog_a_linked_logs_directory_could_carry_elsewhere() {
        let pinned = PINS
            .windows(2)
            .any(|pair| pair == ["-c", "core.logAllRefUpdates=false"]);

        assert!(pinned, "{PINS:?}");
    }

    #[test]
    fn a_gitlink_is_recognised_by_the_mode_git_prints_for_it() {
        assert_eq!(GITLINK_MODE, "160000 ");
    }

    #[cfg(unix)]
    #[test]
    fn a_path_git_printed_is_kept_byte_for_byte() {
        let printed = b"nested/\xff name";

        assert_eq!(native(printed).unwrap().as_os_str().as_bytes(), printed);
    }

    #[test]
    fn a_name_that_is_not_utf8_is_no_name_where_names_are_utf8() {
        assert_eq!(utf8(b"refs/heads/\xff"), None);
        assert_eq!(
            utf8(b"refs/heads/main"),
            Some(PathBuf::from("refs/heads/main"))
        );
    }

    #[cfg(not(unix))]
    #[test]
    fn a_name_git_printed_that_is_not_utf8_names_nothing_here() {
        assert_eq!(native(b"refs/heads/\xff"), None);
    }

    /// What [`LOCATING`] prints for a worktree whose own git directory is
    /// `own` and whose repository's is `shared`.
    fn located(own: &std::path::Path, shared: &std::path::Path) -> Vec<u8> {
        [own, shared]
            .iter()
            .flat_map(|directory| {
                let mut line = directory.as_os_str().as_encoded_bytes().to_vec();
                line.push(b'\n');
                line
            })
            .collect()
    }

    #[test]
    fn real_files_and_directories_at_every_depth_are_accepted() {
        let directory = tempfile::tempdir().unwrap();
        for name in [
            "refs/heads/task",
            "logs/refs/heads/task",
            "objects/info",
            "objects/ab",
        ] {
            std::fs::create_dir_all(directory.path().join(name)).unwrap();
        }
        for name in [
            "HEAD",
            "refs/heads/task/one",
            "logs/refs/heads/task/one",
            "objects/info/alternates",
            "objects/ab/cdef",
        ] {
            std::fs::write(directory.path().join(name), "held\n").unwrap();
        }

        assert!(unlinked(&located(directory.path(), directory.path())).is_ok());
    }

    /// A symbolic link at `name` under `base`, pointing at a file that is
    /// not there, with every directory on the way made.
    #[cfg(unix)]
    fn link(base: &std::path::Path, name: &str) {
        let standing = base.join(name);
        std::fs::create_dir_all(standing.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(base.join("elsewhere"), standing).unwrap();
    }

    /// Where a link stands: in a main worktree's git directory, which is
    /// its own and the shared one at once; in a linked worktree's own, kept
    /// inside the shared one as git keeps it or outside it; or in the shared
    /// one of a linked worktree.
    #[cfg(unix)]
    #[derive(Debug, Clone, Copy)]
    enum Placement {
        Main,
        OwnInside,
        OwnOutside,
        Shared,
    }

    /// Paths git writes by name, and through a link standing at any of them
    /// or at a directory above one, at every depth, with a name no refusal
    /// may carry among them.
    #[cfg(unix)]
    const WRITTEN: [&str; 26] = [
        "HEAD",
        "ORIG_HEAD",
        "FETCH_HEAD",
        "config",
        "index",
        "COMMIT_EDITMSG",
        "packed-refs",
        "refs",
        "refs/heads/main",
        "refs/heads/planted\u{202E}",
        "refs/remotes/origin/main",
        "logs",
        "logs/HEAD",
        "logs/refs/remotes/origin/main",
        "reftable",
        "reftable/tables.list",
        "worktrees",
        "worktrees/two/index",
        "modules/nested/config",
        "objects",
        "objects/info",
        "objects/info/alternates",
        "objects/info/commit-graphs/graph.graph",
        "objects/pack",
        "objects/pack/pack-one.pack",
        "objects/ab",
    ];

    #[cfg(unix)]
    #[test]
    fn a_symbolic_link_anywhere_in_either_git_directory_is_refused_unnamed() {
        for name in WRITTEN {
            for placement in [
                Placement::Main,
                Placement::OwnInside,
                Placement::OwnOutside,
                Placement::Shared,
            ] {
                let shared = tempfile::tempdir().unwrap();
                let outside = tempfile::tempdir().unwrap();
                let own = match placement {
                    Placement::Main => shared.path().to_path_buf(),
                    Placement::OwnInside => shared.path().join("worktrees").join("one"),
                    Placement::OwnOutside | Placement::Shared => outside.path().join("one"),
                };
                std::fs::create_dir_all(&own).unwrap();
                match placement {
                    Placement::Main | Placement::Shared => link(shared.path(), name),
                    Placement::OwnInside | Placement::OwnOutside => link(&own, name),
                }

                let refusal = unlinked(&located(&own, shared.path())).unwrap_err();

                assert!(
                    matches!(refusal, GitError::LinkedPath),
                    "{name:?} {placement:?}: {refusal:?}"
                );
                assert_eq!(
                    refusal.to_string(),
                    "Refusing a repository whose git directory holds a symbolic link"
                );
            }
        }
    }

    /// Loose objects are named by git from their content and run to many
    /// thousands of files, so what stands inside a directory of them is not
    /// looked at; the few packs a clone keeps are.
    #[cfg(unix)]
    #[test]
    fn the_object_store_is_walked_at_its_top_and_in_its_info_and_pack_directories() {
        let loose = tempfile::tempdir().unwrap();
        link(loose.path(), "objects/ab/cdef");
        let packed = tempfile::tempdir().unwrap();
        link(packed.path(), "objects/pack/pack-one.pack");

        let passed = unlinked(&located(loose.path(), loose.path()));
        let refused = unlinked(&located(packed.path(), packed.path()));

        assert!(passed.is_ok(), "{passed:?}");
        assert!(matches!(refused, Err(GitError::LinkedPath)), "{refused:?}");
    }

    #[test]
    fn a_git_directory_that_is_not_there_holds_no_link() {
        let directory = tempfile::tempdir().unwrap();
        let missing = directory.path().join("missing");

        assert!(unlinked(&located(&missing, &missing)).is_ok());
    }

    /// What vanished while the walk ran, or became a file, holds no link,
    /// and what could not be looked at may hold one.
    #[test]
    fn only_what_vanished_is_passed_over() {
        let vanished = present::<()>(Err(std::io::ErrorKind::NotFound.into()));
        let replaced = present::<()>(Err(std::io::ErrorKind::NotADirectory.into()));
        let unreadable = present::<()>(Err(std::io::ErrorKind::PermissionDenied.into()));

        assert!(matches!(vanished, Ok(None)), "{vanished:?}");
        assert!(matches!(replaced, Ok(None)), "{replaced:?}");
        assert!(
            matches!(unreadable, Err(GitError::CommandFailed(_))),
            "{unreadable:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_directory_that_cannot_be_read_is_refused() {
        use std::os::unix::fs::PermissionsExt;
        if nix::unistd::geteuid().is_root() {
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let sealed = directory.path().join("refs");
        std::fs::create_dir(&sealed).unwrap();
        std::fs::set_permissions(&sealed, std::fs::Permissions::from_mode(0o000)).unwrap();

        let walked = unlinked(&located(directory.path(), directory.path()));

        std::fs::set_permissions(&sealed, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(
            matches!(walked, Err(GitError::CommandFailed(_))),
            "{walked:?}"
        );
    }

    #[test]
    fn a_listing_that_is_not_one_absolute_directory_for_each_is_refused() {
        let directory = tempfile::tempdir().unwrap();
        let mut extra = located(directory.path(), directory.path());
        extra.extend_from_slice(b"/one/more\n");
        let relative = located(
            std::path::Path::new("relative"),
            std::path::Path::new("relative"),
        );
        let mut short = directory.path().as_os_str().as_encoded_bytes().to_vec();
        short.push(b'\n');

        for listed in [&extra[..], &relative[..], &short[..], b""] {
            assert!(
                matches!(unlinked(listed), Err(GitError::CommandFailed(_))),
                "{}",
                String::from_utf8_lossy(listed)
            );
        }
    }

    #[test]
    fn only_absolute_entries_of_the_search_path_are_kept() {
        let joined = std::env::join_paths(["/usr/bin", "", ".", "bin", "/bin"]).unwrap();

        assert_eq!(
            std::env::split_paths(&absolute(&joined)).collect::<Vec<_>>(),
            vec![
                std::path::PathBuf::from("/usr/bin"),
                std::path::PathBuf::from("/bin")
            ]
        );
    }

    /// An empty search path, or one with an empty element, sends `execvp`
    /// and every helper git starts to the directory the command runs in,
    /// which a run can write.
    #[cfg(unix)]
    #[test]
    fn a_search_path_with_no_absolute_entry_searches_only_where_nothing_is() {
        for path in ["", ":", ".", "relative", "relative:"] {
            let searched: Vec<PathBuf> =
                std::env::split_paths(&absolute(OsStr::new(path))).collect();

            assert!(
                !searched.is_empty() && searched.iter().all(|entry| entry.is_absolute()),
                "{path:?} searches {searched:?}"
            );
            assert_eq!(searched, vec![PathBuf::from(NOWHERE)], "{path:?}");
            assert_eq!(found(OsStr::new(path)), None, "{path:?}");
        }
    }

    /// Where no entry holds git, the command names a git that cannot exist,
    /// rather than a bare name `execvp` would look for where it runs.
    #[cfg(unix)]
    #[test]
    fn git_no_entry_holds_is_named_where_nothing_is() {
        let named = executable(OsStr::new(""));

        assert_eq!(named, unfound());
        assert!(named.is_absolute(), "{}", named.display());
        assert!(!named.exists(), "{}", named.display());
    }

    /// `directory` named relative to the directory the tests run in.
    #[cfg(unix)]
    fn from_here(directory: &Path) -> PathBuf {
        let here = std::env::current_dir().unwrap();
        let mut relative: PathBuf = here.components().skip(1).map(|_| "..").collect();
        relative.push(directory.strip_prefix("/").unwrap());
        relative
    }

    /// Git is found where `execvp` would find it on the hardened `PATH`:
    /// the first absolute entry holding a `git` file this process may
    /// execute. A relative entry holding one is passed over.
    #[cfg(unix)]
    #[test]
    fn git_is_the_first_executable_one_an_absolute_entry_holds() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempfile::tempdir().unwrap();
        let entry = |name: &str, mode: Option<u32>| {
            let directory = root.path().join(name);
            std::fs::create_dir(&directory).unwrap();
            if let Some(mode) = mode {
                std::fs::write(directory.join(GIT), "").unwrap();
                std::fs::set_permissions(
                    directory.join(GIT),
                    std::fs::Permissions::from_mode(mode),
                )
                .unwrap();
            }
            directory
        };
        let relative = from_here(&entry("relative", Some(0o700)));
        assert!(
            relative.is_relative() && runnable(&relative.join(GIT)),
            "{} names no git this process may run",
            relative.display()
        );
        let missing = entry("missing", None);
        let unexecutable = entry("unexecutable", Some(0o600));
        let nested = entry("nested", None);
        std::fs::create_dir(nested.join(GIT)).unwrap();
        let mut entries = vec![relative, missing, unexecutable, nested];
        if !nix::unistd::geteuid().is_root() {
            entries.push(entry("others", Some(0o001)));
        }
        let first = entry("first", Some(0o700));
        let second = entry("second", Some(0o700));
        entries.extend([first.clone(), second]);

        assert_eq!(
            found(&std::env::join_paths(&entries).unwrap()),
            Some(first.join(GIT))
        );
    }

    /// Set in the re-run of a test that needs its own `PATH`.
    #[cfg(unix)]
    const CHILD: &str = "ABNEGATE_VCS_TEST_CHILD";

    /// Names the directory the re-run's command runs in.
    #[cfg(unix)]
    const PLANTED: &str = "ABNEGATE_VCS_TEST_PLANTED";

    /// A host whose `PATH` holds no absolute entry with git starts no git at
    /// all, not the `git` a run planted in the directory a command runs in.
    #[cfg(unix)]
    #[test]
    fn a_git_planted_where_a_command_runs_is_not_run() {
        use std::os::unix::fs::PermissionsExt;

        const NAME: &str = "git::hardening::tests::a_git_planted_where_a_command_runs_is_not_run";
        if std::env::var(CHILD).as_deref() == Ok(NAME) {
            let directory = PathBuf::from(std::env::var_os(PLANTED).expect("the parent plants it"));
            let started = hardened().current_dir(directory).status();
            assert!(
                matches!(&started, Err(error) if error.kind() == std::io::ErrorKind::NotFound),
                "{started:?}"
            );
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let ran = directory.path().join("ran");
        let planted = directory.path().join(GIT);
        std::fs::write(
            &planted,
            format!("#!/bin/sh\necho ran > '{}'\n", ran.display()),
        )
        .unwrap();
        std::fs::set_permissions(&planted, std::fs::Permissions::from_mode(0o700)).unwrap();

        for path in ["", ":", ".", "relative"] {
            let output = Command::new(std::env::current_exe().expect("the test binary"))
                .args(["--exact", NAME, "--nocapture"])
                .env(CHILD, NAME)
                .env(PLANTED, directory.path())
                .env(SEARCH, path)
                .output()
                .expect("the child runs");

            let printed = String::from_utf8_lossy(&output.stdout);
            assert!(
                output.status.success() && printed.contains("1 passed"),
                "{path:?}: {printed}{}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(!ran.exists(), "a PATH of {path:?} ran the planted git");
        }
    }

    /// Of two absolute entries that each hold a git, a hardened command runs
    /// the first one's, as `execvp` searching the same `PATH` would.
    #[cfg(unix)]
    #[test]
    fn a_hardened_command_runs_the_git_of_the_first_entry_that_holds_one() {
        use std::os::unix::fs::PermissionsExt;

        const NAME: &str = "git::hardening::tests::a_hardened_command_runs_the_git_of_the_first_entry_that_holds_one";
        if std::env::var(CHILD).as_deref() == Ok(NAME) {
            let search = std::env::var_os(SEARCH).expect("the parent sets it");
            let first = std::env::split_paths(&search)
                .next()
                .expect("the parent names two entries");
            assert_eq!(Path::new(hardened().get_program()), first.join(GIT));
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let entries = ["first", "second"].map(|name| {
            let entry = directory.path().join(name);
            std::fs::create_dir(&entry).unwrap();
            std::fs::write(entry.join(GIT), "#!/bin/sh\n").unwrap();
            std::fs::set_permissions(entry.join(GIT), std::fs::Permissions::from_mode(0o700))
                .unwrap();
            entry
        });

        let output = Command::new(std::env::current_exe().expect("the test binary"))
            .args(["--exact", NAME, "--nocapture"])
            .env(CHILD, NAME)
            .env(SEARCH, std::env::join_paths(&entries).unwrap())
            .output()
            .expect("the child runs");

        let printed = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success() && printed.contains("1 passed"),
            "{printed}{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    /// Every hardened command is built by [`hardened`]: git named by the
    /// path it found, the search path it read, and the pins first.
    #[cfg(unix)]
    #[test]
    fn every_hardened_command_is_the_one_builder_s() {
        let built = hardened();
        let program = Path::new(built.get_program());
        assert!(program.is_absolute(), "{}", program.display());
        assert!(runnable(program), "{}", program.display());
        let search = |command: &Command| {
            command
                .get_envs()
                .find(|(key, _)| *key == SEARCH)
                .and_then(|(_, value)| value.map(OsStr::to_os_string))
        };
        let remote =
            crate::repository_url::RepositoryUrl::parse("https://github.com/owner/repository")
                .unwrap();

        for (name, command) in [
            ("hardened", crate::git::GitService::hardened().as_std()),
            (
                "connected",
                crate::git::GitService::connected(&remote, None).as_std(),
            ),
            ("local", &crate::worktree::local(Path::new("."))),
            ("origin", &crate::git::GitService::origin_listing()),
        ] {
            assert_eq!(command.get_program(), built.get_program(), "{name}");
            assert_eq!(search(command), search(&built), "{name}");
            assert!(
                command
                    .get_args()
                    .take(PINS.len())
                    .eq(PINS.iter().map(OsStr::new)),
                "{name}"
            );
        }
    }

    #[test]
    fn every_hardened_command_carries_the_pins_and_environment() {
        let command = hardened();
        let arguments: Vec<String> = command
            .get_args()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect();
        let environment: Vec<(String, Option<String>)> = command
            .get_envs()
            .map(|(key, value)| {
                (
                    key.to_string_lossy().into_owned(),
                    value.map(|value| value.to_string_lossy().into_owned()),
                )
            })
            .collect();

        for pin in PINS {
            assert!(arguments.contains(&pin.to_string()), "{pin}");
        }
        for (key, value) in ENVIRONMENT {
            assert!(
                environment.contains(&(key.to_string(), Some(value.to_string()))),
                "{key}={value}"
            );
        }
        assert!(
            !arguments.iter().any(|argument| argument.contains("sslCA")),
            "an empty CA setting breaks every HTTPS request"
        );
    }
}
