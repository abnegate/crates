use std::collections::BTreeMap;
use std::env;
use std::ffi::OsString;

/// The names [`EnvironmentPolicy::default`] copies from the executor into
/// every command: where to find programs, the user's home and scratch space,
/// locale, terminal and time zone, the trusted certificates, and the proxy a
/// deployment routes through.
pub const DEFAULT_ENVIRONMENT_ALLOWLIST: &[&str] = &[
    "PATH",
    "HOME",
    "TMPDIR",
    "LANG",
    "LC_ALL",
    "TERM",
    "TZ",
    "SSL_CERT_FILE",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "NO_PROXY",
    "http_proxy",
    "https_proxy",
    "no_proxy",
];

/// Which of the executor's own environment variables a spawned command sees.
///
/// The executor usually holds credentials a command must never read, so the
/// default is [`EnvironmentPolicy::Allowlist`] of
/// [`DEFAULT_ENVIRONMENT_ALLOWLIST`]. Whatever the policy passes on is read
/// from the executor when the command spawns, and the `RunStart.env` map is
/// layered on top of it.
///
/// A confined command additionally has its sandbox's own `HOME`, `TMPDIR`,
/// `TMP`, `TEMP`, `PATH`, `LANG` and `LC_ALL`, which take precedence over
/// anything the policy passes on; only `RunStart.env` overrides those.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum EnvironmentPolicy {
    /// Pass on only these names, each only when the executor has it set.
    Allowlist(Vec<String>),
    /// Pass on the executor's whole environment. An explicit opt-in for an
    /// executor whose environment holds nothing a command must not see.
    Inherit,
}

impl Default for EnvironmentPolicy {
    fn default() -> Self {
        Self::Allowlist(
            DEFAULT_ENVIRONMENT_ALLOWLIST
                .iter()
                .map(|name| name.to_string())
                .collect(),
        )
    }
}

impl EnvironmentPolicy {
    /// The executor's variables this policy passes on, read now.
    pub fn inherited(&self) -> BTreeMap<OsString, OsString> {
        match self {
            Self::Allowlist(names) => names
                .iter()
                .filter_map(|name| env::var_os(name).map(|value| (OsString::from(name), value)))
                .collect(),
            Self::Inherit => env::vars_os().collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_is_the_documented_allowlist() {
        let EnvironmentPolicy::Allowlist(names) = EnvironmentPolicy::default() else {
            panic!("the default is an allowlist");
        };

        assert_eq!(names, DEFAULT_ENVIRONMENT_ALLOWLIST);
        for name in ["PATH", "HOME", "TMPDIR", "TERM", "HTTPS_PROXY", "no_proxy"] {
            assert!(names.iter().any(|allowed| allowed == name), "{name}");
        }
    }

    #[test]
    fn an_allowlist_passes_on_only_the_names_it_lists() {
        let policy = EnvironmentPolicy::Allowlist(vec!["PATH".to_string()]);

        let inherited = policy.inherited();

        assert_eq!(
            inherited.keys().collect::<Vec<&OsString>>(),
            [&OsString::from("PATH")]
        );
        assert_eq!(
            inherited.get(&OsString::from("PATH")),
            env::var_os("PATH").as_ref()
        );
    }

    #[test]
    fn an_allowlist_skips_a_name_the_executor_does_not_have() {
        let policy =
            EnvironmentPolicy::Allowlist(vec!["ABNEGATE_EXEC_NEVER_SET_ANYWHERE".to_string()]);

        assert!(policy.inherited().is_empty());
    }

    #[test]
    fn inherit_passes_on_everything() {
        assert_eq!(
            EnvironmentPolicy::Inherit.inherited(),
            env::vars_os().collect::<BTreeMap<OsString, OsString>>()
        );
    }
}
