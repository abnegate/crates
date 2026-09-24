use std::collections::BTreeMap;

use abnegate_secret::SecretValue;
use tokio::process::Command;

/// The names a default [`EnvironmentPolicy`] copies from this process.
///
/// Enough to find programs, a home and a scratch directory, to know which user
/// is running them - tools backed by the macOS keychain look the account up by
/// `USER` - and to decode text; nothing that carries a credential.
pub const DEFAULT_ENVIRONMENT: &[&str] = &[
    "PATH", "HOME", "USER", "LOGNAME", "TMPDIR", "LANG", "LC_ALL", "TERM",
];

/// The environment a spawned child is given.
///
/// An allowlist by default: the child sees the variables named here and
/// nothing else, because the process spawning it holds database URLs, signing
/// keys and provider keys that a child would print straight into tool output.
/// [`inherit`](Self::inherit) is the explicit opt-in to the whole of this
/// process's environment.
///
/// Values are held as [`SecretValue`]s, so a policy - and any context or spec
/// carrying one - can be debug-printed without printing a credential.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvironmentPolicy {
    variables: BTreeMap<String, SecretValue>,
    inherit: bool,
}

impl EnvironmentPolicy {
    /// No variables at all.
    pub fn empty() -> Self {
        Self {
            variables: BTreeMap::new(),
            inherit: false,
        }
    }

    /// Each [`DEFAULT_ENVIRONMENT`] name this process has, with its value.
    pub fn allowlist() -> Self {
        DEFAULT_ENVIRONMENT
            .iter()
            .filter_map(|name| std::env::var(name).ok().map(|value| (*name, value)))
            .collect()
    }

    /// This process's whole environment, read as each child starts, with any
    /// variables set here laid over it.
    pub fn inherit() -> Self {
        Self::empty().inheriting()
    }

    /// The same variables, laid over this process's whole environment.
    pub fn inheriting(mut self) -> Self {
        self.inherit = true;
        self
    }

    /// Whether a child also sees this process's whole environment.
    pub fn inherits(&self) -> bool {
        self.inherit
    }

    pub fn with(mut self, name: impl Into<String>, value: impl Into<SecretValue>) -> Self {
        self.set(name, value);
        self
    }

    pub fn set(&mut self, name: impl Into<String>, value: impl Into<SecretValue>) {
        self.variables.insert(name.into(), value.into());
    }

    pub fn remove(&mut self, name: &str) -> Option<SecretValue> {
        self.variables.remove(name)
    }

    pub fn get(&self, name: &str) -> Option<&SecretValue> {
        self.variables.get(name)
    }

    pub fn contains(&self, name: &str) -> bool {
        self.variables.contains_key(name)
    }

    /// The names set here, in order.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.variables.keys().map(String::as_str)
    }

    /// Give `command` exactly this environment.
    pub fn apply(&self, command: &mut Command) {
        if !self.inherit {
            command.env_clear();
        }
        for (name, value) in &self.variables {
            command.env(name, value.expose());
        }
    }
}

/// The [`allowlist`](EnvironmentPolicy::allowlist).
impl Default for EnvironmentPolicy {
    fn default() -> Self {
        Self::allowlist()
    }
}

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
    use super::*;

    /// A variable this process has that the allowlist does not name. Cargo
    /// sets several on every test binary it runs.
    fn unlisted() -> Option<(String, String)> {
        std::env::vars().find(|(name, _)| !DEFAULT_ENVIRONMENT.contains(&name.as_str()))
    }

    #[test]
    fn the_default_is_the_allowlist() {
        assert_eq!(EnvironmentPolicy::default(), EnvironmentPolicy::allowlist());
        assert!(!EnvironmentPolicy::default().inherits());
        assert_eq!(EnvironmentPolicy::empty().names().count(), 0);
    }

    #[test]
    fn the_allowlist_copies_only_the_names_it_lists() {
        let policy = EnvironmentPolicy::allowlist();
        for name in policy.names() {
            assert!(DEFAULT_ENVIRONMENT.contains(&name), "{name}");
        }
        if let Ok(path) = std::env::var("PATH") {
            assert_eq!(
                policy.get("PATH").map(SecretValue::expose),
                Some(path.as_str())
            );
        }
        if let Some((name, _)) = unlisted() {
            assert!(!policy.contains(&name), "{name} was copied");
        }
    }

    /// Without `USER` a keychain-backed tool cannot tell whose keychain to
    /// open, and fails as though it had no credentials at all.
    #[test]
    fn the_allowlist_says_which_user_is_running() {
        for name in ["USER", "LOGNAME"] {
            assert!(DEFAULT_ENVIRONMENT.contains(&name), "{name}");
            if let Ok(value) = std::env::var(name) {
                assert_eq!(
                    EnvironmentPolicy::allowlist()
                        .get(name)
                        .map(SecretValue::expose),
                    Some(value.as_str()),
                    "{name}"
                );
            }
        }
    }

    #[test]
    fn debug_names_the_variables_and_never_prints_a_value() {
        let policy = EnvironmentPolicy::empty().with("API_TOKEN", "hunter2-secret");
        let printed = format!("{policy:?}");
        assert!(printed.contains("API_TOKEN"), "{printed}");
        assert!(!printed.contains("hunter2-secret"), "{printed}");
    }

    #[tokio::test]
    async fn a_child_sees_the_allowlist_and_nothing_else_unless_it_inherits() {
        let Some((name, value)) = unlisted() else {
            return;
        };
        let read = |policy: EnvironmentPolicy| async move {
            let mut command = Command::new("/usr/bin/env");
            policy.apply(&mut command);
            let output = command.output().await.expect("env runs");
            String::from_utf8_lossy(&output.stdout).into_owned()
        };

        let listed = read(EnvironmentPolicy::allowlist().with("EXTRA", "1")).await;
        assert!(listed.lines().any(|line| line == "EXTRA=1"), "{listed}");
        assert!(
            !listed
                .lines()
                .any(|line| line.starts_with(&format!("{name}="))),
            "{name} reached the child: {listed}"
        );

        let inherited = read(EnvironmentPolicy::inherit()).await;
        assert!(
            inherited
                .lines()
                .any(|line| line == format!("{name}={value}")),
            "{inherited}"
        );
    }
}
