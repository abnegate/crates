use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::env;
use std::ffi::OsString;

use abnegate_secret::SecretValue;
use tokio::process::Command;

/// The names [`EnvironmentPolicy::allowlist`] passes from the executor to
/// every command: where to find programs, the user's home, name and scratch
/// space, locale and terminal, the timezone (`TZ`), and the trusted
/// certificates.
///
/// No proxy variable is among them. A proxy URL can carry credentials, so a
/// command reaches a proxy only through [`Proxy`](crate::Proxy) or a name
/// passed to [`EnvironmentPolicy::allow`].
pub const DEFAULT_ENVIRONMENT: &[&str] = &[
    "PATH",
    "HOME",
    "USER",
    "LOGNAME",
    "TMPDIR",
    "LANG",
    "LC_ALL",
    "TERM",
    "TZ",
    "SSL_CERT_FILE",
];

/// The environment a spawned command is given.
///
/// Three layers, each over the one before:
///
/// - the executor's whole environment, only when the policy
///   [inherits](Self::inherits);
/// - the [allowed](Self::allow) names, each with the executor's own value,
///   passed only when the executor has it set;
/// - the variables [set](Self::set) here.
///
/// Nothing is read from the executor until a command spawns, so an allowed
/// name follows whatever the executor holds at that moment. The default is
/// [`allowlist`](Self::allowlist), because the executor usually holds
/// credentials a command must never read; [`inherit`](Self::inherit) is the
/// explicit opt-in to its whole environment.
///
/// Values set here are held as [`SecretValue`]s, so `Debug` prints their names
/// and never their values.
///
/// A confined command additionally has its sandbox's own `HOME`, `TMPDIR`,
/// `TMP`, `TEMP`, `PATH`, `LANG` and `LC_ALL`, which take precedence over
/// anything the policy passes on; only
/// [`RunStart::environment`](crate::protocol::RunStart::environment) overrides
/// those.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvironmentPolicy {
    allowed: BTreeSet<String>,
    variables: BTreeMap<String, SecretValue>,
    inherit: bool,
}

impl EnvironmentPolicy {
    /// No variables at all.
    pub fn empty() -> Self {
        Self {
            allowed: BTreeSet::new(),
            variables: BTreeMap::new(),
            inherit: false,
        }
    }

    /// The [`DEFAULT_ENVIRONMENT`] names, each with the executor's value.
    pub fn allowlist() -> Self {
        Self::empty().allow(DEFAULT_ENVIRONMENT.iter().copied())
    }

    /// The same policy, also passing `names` from the executor, each only
    /// when the executor has it set.
    pub fn allow<I, S>(mut self, names: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.allowed.extend(names.into_iter().map(Into::into));
        self
    }

    /// The executor's whole environment, read as each command spawns.
    pub fn inherit() -> Self {
        Self::empty().inheriting()
    }

    /// The same policy, laid over the executor's whole environment.
    pub fn inheriting(mut self) -> Self {
        self.inherit = true;
        self
    }

    /// Whether a command also sees the executor's whole environment.
    pub fn inherits(&self) -> bool {
        self.inherit
    }

    /// The same policy, with `name` set to `value` over everything else.
    pub fn with(mut self, name: impl Into<String>, value: impl Into<SecretValue>) -> Self {
        self.set(name, value);
        self
    }

    /// Set `name` to `value` over everything else.
    pub fn set(&mut self, name: impl Into<String>, value: impl Into<SecretValue>) {
        self.variables.insert(name.into(), value.into());
    }

    /// Stop passing `name`: forget the value set here and stop allowing the
    /// executor's own. Returns the value that was set here, if any.
    ///
    /// A policy that [inherits](Self::inherits) still passes the executor's
    /// whole environment, `name` included when the executor has it.
    pub fn remove(&mut self, name: &str) -> Option<SecretValue> {
        self.allowed.remove(name);
        self.variables.remove(name)
    }

    /// The value set here for `name` or, for an allowed name, the executor's
    /// own value read now.
    ///
    /// A name the policy passes only because it inherits is never read, nor
    /// is a value that is not UTF-8.
    pub fn get(&self, name: &str) -> Option<SecretValue> {
        match self.variables.get(name) {
            Some(value) => Some(value.clone()),
            None if self.allowed.contains(name) => env::var(name).ok().map(SecretValue::new),
            None => None,
        }
    }

    /// Whether `name` is set here, or is allowed and set in the executor now.
    ///
    /// Never true of a name only because the policy inherits.
    pub fn contains(&self, name: &str) -> bool {
        self.variables.contains_key(name) || self.passes_on(name)
    }

    /// The names set here and the allowed names the executor has set now,
    /// in order, each once.
    ///
    /// Never the rest of an inherited environment.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.variables
            .keys()
            .map(String::as_str)
            .chain(
                self.allowed
                    .iter()
                    .map(String::as_str)
                    .filter(|name| self.passes_on(name)),
            )
            .collect::<BTreeSet<&str>>()
            .into_iter()
    }

    /// Every variable this policy gives a command, read now: the executor's
    /// whole environment when it inherits, then the allowed names the
    /// executor has set, then the variables set here.
    pub fn inherited(&self) -> BTreeMap<OsString, OsString> {
        let mut inherited: BTreeMap<OsString, OsString> = if self.inherit {
            env::vars_os().collect()
        } else {
            BTreeMap::new()
        };
        inherited.extend(self.allowed_values());
        inherited.extend(
            self.variables
                .iter()
                .map(|(name, value)| (OsString::from(name), OsString::from(value.expose()))),
        );
        inherited
    }

    /// Give `command` exactly this policy's environment: cleared unless the
    /// policy inherits, then the allowed names the executor has set, then the
    /// variables set here.
    pub fn apply(&self, command: &mut Command) {
        if !self.inherit {
            command.env_clear();
        }
        command.envs(self.allowed_values());
        for (name, value) in &self.variables {
            command.env(name, value.expose());
        }
    }

    fn passes_on(&self, name: &str) -> bool {
        self.allowed.contains(name) && env::var_os(name).is_some()
    }

    fn allowed_values(&self) -> impl Iterator<Item = (OsString, OsString)> + '_ {
        self.allowed
            .iter()
            .filter_map(|name| env::var_os(name).map(|value| (OsString::from(name), value)))
    }
}

/// The [`allowlist`](EnvironmentPolicy::allowlist).
impl Default for EnvironmentPolicy {
    fn default() -> Self {
        Self::allowlist()
    }
}

/// Variables set here, with nothing allowed from the executor.
impl<Name: Into<String>, Value: Into<SecretValue>> FromIterator<(Name, Value)>
    for EnvironmentPolicy
{
    fn from_iter<Variables: IntoIterator<Item = (Name, Value)>>(variables: Variables) -> Self {
        let mut policy = Self::empty();
        for (name, value) in variables {
            policy.set(name, value);
        }
        policy
    }
}

#[cfg(test)]
mod tests {
    use crate::executor::child;

    use super::*;

    const ALLOWED: &str = "ABNEGATE_EXEC_TEST_ALLOWED";
    const UNLISTED: &str = "ABNEGATE_EXEC_TEST_UNLISTED";

    async fn environment_of(policy: &EnvironmentPolicy) -> Vec<String> {
        let mut command = Command::new("/usr/bin/env");
        policy.apply(&mut command);
        let output = command.output().await.expect("env runs");
        let mut lines: Vec<String> = String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(str::to_string)
            .collect();
        lines.sort();
        lines
    }

    fn names(policy: &EnvironmentPolicy) -> Vec<&str> {
        policy.names().collect()
    }

    #[test]
    fn the_default_is_the_allowlist_of_the_default_names() {
        let policy = EnvironmentPolicy::default();

        assert_eq!(policy, EnvironmentPolicy::allowlist());
        assert!(!policy.inherits());
        assert!(policy.variables.is_empty());
        assert!(
            policy.allowed.iter().eq(DEFAULT_ENVIRONMENT
                .iter()
                .copied()
                .collect::<BTreeSet<&str>>())
        );
    }

    /// Without `USER` a keychain-backed tool cannot tell whose keychain to
    /// open, without `TZ` a command reads the clock as UTC, and without
    /// `SSL_CERT_FILE` it cannot verify a server behind a private certificate
    /// authority.
    #[test]
    fn the_allowlist_covers_the_user_the_timezone_and_the_certificates() {
        let policy = EnvironmentPolicy::allowlist();

        for name in ["USER", "LOGNAME", "TZ", "SSL_CERT_FILE"] {
            assert!(DEFAULT_ENVIRONMENT.contains(&name), "{name}");
            assert!(policy.allowed.contains(name), "{name}");
        }
    }

    #[test]
    fn no_proxy_variable_is_allowed_by_default() {
        for name in DEFAULT_ENVIRONMENT {
            assert!(!name.to_ascii_uppercase().ends_with("_PROXY"), "{name}");
        }
    }

    #[test]
    fn debug_names_the_variables_and_never_prints_a_value() {
        let policy = EnvironmentPolicy::allowlist()
            .with("API_TOKEN", "hunter2-secret")
            .inheriting();

        let printed = format!("{policy:?}");

        assert!(printed.contains("API_TOKEN"), "{printed}");
        assert!(printed.contains("PATH"), "{printed}");
        assert!(!printed.contains("hunter2-secret"), "{printed}");
    }

    #[test]
    fn collecting_variables_sets_them_and_allows_nothing() {
        let policy = EnvironmentPolicy::from_iter([("EXTRA", "1")]);

        assert!(policy.allowed.is_empty());
        assert!(!policy.inherits());
        assert_eq!(names(&policy), ["EXTRA"]);
        assert_eq!(
            policy.get("EXTRA").as_ref().map(SecretValue::expose),
            Some("1")
        );
        assert!(!policy.contains("PATH"));
    }

    #[test]
    fn a_value_set_here_outranks_the_executors() {
        let policy = EnvironmentPolicy::allowlist().with("PATH", "/policy/bin");

        assert_eq!(
            policy.get("PATH").as_ref().map(SecretValue::expose),
            Some("/policy/bin")
        );
        assert_eq!(
            policy.inherited().get(&OsString::from("PATH")),
            Some(&OsString::from("/policy/bin"))
        );
    }

    /// Runs in a child test process that starts with [`ALLOWED`] and
    /// [`UNLISTED`] set, so what the policy reads from its executor is known.
    #[tokio::test]
    async fn an_allowed_name_is_read_from_the_executor_and_an_unlisted_one_never_is() {
        const NAME: &str = "executor::environment_policy::tests::an_allowed_name_is_read_from_the_executor_and_an_unlisted_one_never_is";
        if child::delegated(
            NAME,
            &[(ALLOWED, "allowed-value"), (UNLISTED, "unlisted-value")],
        )
        .await
        {
            return;
        }
        let listed = EnvironmentPolicy::empty()
            .allow([ALLOWED, "ABNEGATE_EXEC_NEVER_SET_ANYWHERE"])
            .with("EXTRA", "1");

        for policy in [listed.clone(), listed.clone().inheriting()] {
            assert_eq!(names(&policy), [ALLOWED, "EXTRA"], "{policy:?}");
            assert!(policy.contains(ALLOWED));
            assert_eq!(
                policy.get(ALLOWED).as_ref().map(SecretValue::expose),
                Some("allowed-value")
            );
            assert!(!policy.contains(UNLISTED), "{policy:?}");
            assert!(policy.get(UNLISTED).is_none(), "{policy:?}");
            assert!(!policy.contains("ABNEGATE_EXEC_NEVER_SET_ANYWHERE"));
        }

        assert_eq!(
            listed.inherited().into_keys().collect::<Vec<OsString>>(),
            [OsString::from(ALLOWED), OsString::from("EXTRA")]
        );
        assert!(
            listed
                .clone()
                .inheriting()
                .inherited()
                .contains_key(&OsString::from(UNLISTED))
        );
    }

    #[tokio::test]
    async fn apply_clears_the_environment_unless_the_policy_inherits() {
        const NAME: &str = "executor::environment_policy::tests::apply_clears_the_environment_unless_the_policy_inherits";
        if child::delegated(
            NAME,
            &[(ALLOWED, "allowed-value"), (UNLISTED, "unlisted-value")],
        )
        .await
        {
            return;
        }
        let policy = EnvironmentPolicy::empty()
            .allow([ALLOWED])
            .with("EXTRA", "1");

        assert_eq!(
            environment_of(&policy).await,
            [format!("{ALLOWED}=allowed-value"), "EXTRA=1".to_string()]
        );

        let inherited = environment_of(&policy.inheriting()).await;
        for line in [
            format!("{ALLOWED}=allowed-value"),
            format!("{UNLISTED}=unlisted-value"),
            "EXTRA=1".to_string(),
        ] {
            assert!(inherited.contains(&line), "{line}: {inherited:?}");
        }
    }

    #[tokio::test]
    async fn remove_stops_passing_a_name_that_was_allowed_and_set() {
        const NAME: &str = "executor::environment_policy::tests::remove_stops_passing_a_name_that_was_allowed_and_set";
        if child::delegated(NAME, &[(ALLOWED, "allowed-value")]).await {
            return;
        }
        let mut policy = EnvironmentPolicy::empty()
            .allow([ALLOWED])
            .with(ALLOWED, "overlay-value")
            .with("EXTRA", "1");

        let removed = policy.remove(ALLOWED);

        assert_eq!(
            removed.as_ref().map(SecretValue::expose),
            Some("overlay-value")
        );
        assert!(!policy.contains(ALLOWED));
        assert!(policy.get(ALLOWED).is_none());
        assert_eq!(names(&policy), ["EXTRA"]);
        assert_eq!(environment_of(&policy).await, ["EXTRA=1"]);
        assert!(policy.remove(ALLOWED).is_none());
    }
}
