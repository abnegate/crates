use std::process::Command;
use std::process::Stdio;

/// Settings pinned on every command line, where a repository's own
/// configuration cannot override them: nothing a run wrote into it may run a
/// hook, a file-system monitor, a credential helper or a signing program,
/// follow a redirect, turn off certificate checks, route through a proxy or
/// mark files so a status stops seeing them.
///
/// `http.sslCAInfo` and `http.sslCAPath` are deliberately absent: git hands an
/// empty value to curl verbatim and every HTTPS request then fails. A
/// repository that sets either is refused by [`refused`] instead.
const PINS: [&str; 20] = [
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

/// Repository configuration sections that name a program, a transport rewrite,
/// an HTTP override or another file to read. A run's git commands can write
/// them, and none can be pinned away on the command line.
const REFUSED_SECTIONS: [&str; 7] = [
    "url.",
    "http.",
    "include.",
    "includeif.",
    "filter.",
    "diff.",
    "merge.",
];

/// Single repository configuration keys refused for the same reason: an SSH
/// program, per-worktree configuration nothing here reads, and a work tree
/// somewhere other than where the command was pointed.
const REFUSED_KEYS: [&str; 3] = [
    "core.sshcommand",
    "extensions.worktreeconfig",
    "core.worktree",
];

/// The section configuring a named remote, which a URL given on the command
/// line is looked up in as a name.
const REMOTE_SECTION: &str = "remote.";

/// Lists the repository's own configuration keys, NUL-terminated.
pub(crate) const CONFIG_LISTING: [&str; 5] = ["config", "--local", "--name-only", "--list", "-z"];

/// Apply the pins and environment every hardened git command runs with.
pub(crate) fn harden(command: &mut Command) {
    command
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .envs(ENVIRONMENT)
        .args(PINS)
        .stdin(Stdio::null());
}

/// The first key of a [`CONFIG_LISTING`] a hardened command must not run
/// under, if there is one.
pub(crate) fn refused(listing: &[u8]) -> Option<String> {
    listing
        .split(|byte| *byte == 0)
        .map(|key| String::from_utf8_lossy(key).to_ascii_lowercase())
        .find(|key| {
            REFUSED_SECTIONS
                .iter()
                .any(|section| key.starts_with(section))
                || REFUSED_KEYS.contains(&key.as_str())
                || names_a_location(key)
        })
}

/// Whether a `remote.<name>.<key>` entry is named like a URL, which git
/// applies to a push or fetch given that URL on the command line.
fn names_a_location(key: &str) -> bool {
    key.strip_prefix(REMOTE_SECTION)
        .and_then(|rest| rest.rsplit_once('.'))
        .is_some_and(|(name, _)| name.contains([':', '/']))
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
                "remote.origin.url",
                "remote.origin.fetch",
                "branch.main.remote",
                "branch.main.merge",
                "extensions.objectformat",
            ])),
            None
        );
    }

    #[test]
    fn a_key_that_names_a_program_a_transport_or_another_file_is_refused() {
        for key in [
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
            "extensions.worktreeConfig",
            "core.worktree",
            "remote.https://github.com/owner/repository.git.pushurl",
            "remote.file:///tmp/x.receivepack",
        ] {
            assert_eq!(
                refused(&listing(&["core.bare", key])),
                Some(key.to_ascii_lowercase()),
                "{key}"
            );
        }
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
