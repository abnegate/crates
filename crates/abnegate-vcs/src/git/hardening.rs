use crate::git::GitError;
use crate::git::GitResult;
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

/// Apply the pins and environment every hardened git command runs with.
pub(crate) fn harden(command: &mut Command) {
    command
        .env_clear()
        .env(
            "PATH",
            absolute(&std::env::var_os("PATH").unwrap_or_default()),
        )
        .envs(ENVIRONMENT)
        .args(PINS)
        .stdin(Stdio::null());
}

/// The absolute entries of a `PATH`. A relative or empty entry is resolved
/// against the directory a command runs in, which for a hardened command is a
/// repository a run can write, so a `git` there would run instead of git.
fn absolute(path: &OsStr) -> OsString {
    std::env::join_paths(std::env::split_paths(path).filter(|entry| entry.is_absolute()))
        .unwrap_or_default()
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
/// vanished meanwhile, and a refusal when it could not look.
fn present<T>(looked: std::io::Result<T>) -> GitResult<Option<T>> {
    match looked {
        Ok(found) => Ok(Some(found)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
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

    /// What vanished while the walk ran holds no link, and what could not
    /// be looked at may hold one.
    #[test]
    fn only_what_vanished_is_passed_over() {
        let vanished = present::<()>(Err(std::io::ErrorKind::NotFound.into()));
        let unreadable = present::<()>(Err(std::io::ErrorKind::PermissionDenied.into()));

        assert!(matches!(vanished, Ok(None)), "{vanished:?}");
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

    #[test]
    fn every_hardened_command_carries_the_pins_and_environment() {
        let mut command = Command::new("git");
        harden(&mut command);
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
