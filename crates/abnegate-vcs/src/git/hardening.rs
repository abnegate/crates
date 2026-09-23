use crate::git::GitError;
use crate::git::GitResult;
use std::ffi::OsStr;
use std::ffi::OsString;
#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;
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
const PERMITTED_KEYS: [&str; 9] = [
    "core.repositoryformatversion",
    "core.filemode",
    "core.bare",
    "core.logallrefupdates",
    "core.ignorecase",
    "core.precomposeunicode",
    "core.symlinks",
    "extensions.objectformat",
    "extensions.refstorage",
];

/// A remote's keys that name where it is and what to fetch from it.
const PERMITTED_REMOTE_KEYS: [&str; 2] = ["url", "fetch"];

/// A branch's keys that name what it tracks.
const PERMITTED_BRANCH_KEYS: [&str; 2] = ["remote", "merge"];

/// The section configuring a named remote.
const REMOTE_SECTION: &str = "remote.";

/// The section configuring a local branch.
const BRANCH_SECTION: &str = "branch.";

/// Paths under a worktree's own git directory, and under the directory
/// every worktree of the repository shares, that git writes by name and
/// through a symbolic link standing in the place of any of them: a ref, a
/// rewrite of the packed refs, a reflog, `FETCH_HEAD`, `ORIG_HEAD`, `HEAD`
/// or a change to the configuration lands wherever the link points. Each
/// worktree keeps its own `HEAD`, `ORIG_HEAD` and `FETCH_HEAD`.
const UNLINKED: [(Directory, &str); 11] = [
    (Directory::Shared, "packed-refs"),
    (Directory::Shared, "refs"),
    (Directory::Shared, "refs/heads"),
    (Directory::Shared, "refs/remotes"),
    (Directory::Shared, "refs/remotes/origin"),
    (Directory::Shared, "refs/tags"),
    (Directory::Shared, "logs"),
    (Directory::Own, "FETCH_HEAD"),
    (Directory::Own, "ORIG_HEAD"),
    (Directory::Own, "HEAD"),
    (Directory::Shared, "config"),
];

/// Which git directory a path in [`UNLINKED`] lives under.
#[derive(Clone, Copy)]
enum Directory {
    /// The worktree's own, which is the repository's for its main worktree.
    Own,
    /// The one every worktree of the repository shares.
    Shared,
}

/// Prints the worktree's own git directory and the one every worktree of
/// its repository shares, a line each, as absolute paths. Git resolves every
/// link on the way to an absolute path it prints, so the paths under them
/// are joined here rather than asked for: a path git printed for a link
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

/// Refuse a repository with a symbolic link standing at any of
/// [`UNLINKED`], given the directories [`LOCATING`] printed, with
/// [`GitError::LinkedPath`]. A path nothing stands at is fine. One that
/// cannot be looked at, and a listing that is not one absolute directory
/// for each, are refused: what stands there cannot be known.
pub(crate) fn unlinked(located: &[u8]) -> GitResult<()> {
    let unlocated = || GitError::CommandFailed("Cannot locate the repository's files".to_string());
    let directories: Vec<PathBuf> = located
        .strip_suffix(b"\n")
        .unwrap_or(located)
        .split(|byte| *byte == b'\n')
        .map(|line| native(line).filter(|directory| directory.is_absolute()))
        .collect::<Option<_>>()
        .ok_or_else(unlocated)?;
    let [own, shared] = directories.as_slice() else {
        return Err(unlocated());
    };
    for (directory, name) in UNLINKED {
        let base = match directory {
            Directory::Own => own,
            Directory::Shared => shared,
        };
        match std::fs::symlink_metadata(base.join(name)) {
            Ok(details) if details.file_type().is_symlink() => {
                return Err(GitError::LinkedPath(name));
            }
            Ok(_) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
                ) => {}
            Err(_) => return Err(unlocated()),
        }
    }
    Ok(())
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
    fn paths_nothing_stands_at_and_real_files_and_directories_are_accepted() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(directory.path().join("refs/heads")).unwrap();
        std::fs::write(directory.path().join("HEAD"), "ref: refs/heads/main\n").unwrap();
        std::fs::write(directory.path().join("refs/remotes"), "not a directory\n").unwrap();

        assert!(unlinked(&located(directory.path(), directory.path())).is_ok());
    }

    /// A symbolic link at `name` under `own` when `in_own`, and under
    /// `shared` otherwise, pointing at a file that is not there.
    #[cfg(unix)]
    fn link(own: &std::path::Path, shared: &std::path::Path, in_own: bool, name: &str) {
        let standing = match in_own {
            true => own.join(name),
            false => shared.join(name),
        };
        std::fs::create_dir_all(standing.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(shared.join("elsewhere"), standing).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_symbolic_link_at_any_path_is_refused_by_its_name() {
        for (directory, name) in UNLINKED {
            let shared = tempfile::tempdir().unwrap();
            let own = shared.path().join("worktrees").join("one");
            link(
                &own,
                shared.path(),
                matches!(directory, Directory::Own),
                name,
            );

            let refusal = unlinked(&located(&own, shared.path())).unwrap_err();

            assert!(
                matches!(refusal, GitError::LinkedPath(refused) if refused == name),
                "{name}: {refusal:?}"
            );
            assert_eq!(
                refusal.to_string(),
                format!("Refusing a repository whose {name} is a symbolic link")
            );
        }
    }

    /// A worktree keeps its own `HEAD`, `ORIG_HEAD` and `FETCH_HEAD` and
    /// shares the rest, so each is looked for in the one directory git
    /// reads it from, and a link in the other is not what git follows.
    #[cfg(unix)]
    #[test]
    fn each_path_is_looked_for_where_git_keeps_it() {
        for (directory, name) in UNLINKED {
            let shared = tempfile::tempdir().unwrap();
            let own = shared.path().join("worktrees").join("one");
            std::fs::create_dir_all(&own).unwrap();
            link(
                &own,
                shared.path(),
                matches!(directory, Directory::Shared),
                name,
            );

            assert!(unlinked(&located(&own, shared.path())).is_ok(), "{name}");
        }
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
