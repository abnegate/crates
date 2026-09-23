use std::ffi::OsStr;
use std::ffi::OsString;
use std::process::Command;
use std::process::Stdio;

/// Settings pinned on every command line, where a repository's own
/// configuration cannot override them: nothing a run wrote into it may run a
/// hook, a file-system monitor, a credential helper, a signing program or a
/// maintenance job, follow a redirect, turn off certificate checks, route
/// through a proxy, recurse into submodules, push tags or options nobody
/// named, mark files so a status stops seeing them, or have a checkout or
/// switch enter a nested repository to list its changes.
///
/// `http.sslCAInfo` and `http.sslCAPath` are deliberately absent: git hands an
/// empty value to curl verbatim and every HTTPS request then fails. A
/// repository that sets either is refused by [`refused`] instead.
pub(crate) const PINS: [&str; 36] = [
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
    fn a_gitlink_is_recognised_by_the_mode_git_prints_for_it() {
        assert_eq!(GITLINK_MODE, "160000 ");
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
