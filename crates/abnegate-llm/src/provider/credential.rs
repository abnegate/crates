//! How a provider is authenticated.

use abnegate_secret::SecretValue;

/// The credential a provider authenticates with.
///
/// A coding agent CLI is usually already signed in on the host, and handing it
/// a key it did not ask for is worse than handing it nothing: the key ends up
/// in a child environment that the agent may echo into its own logs. So
/// [`Credential::Inherited`] is a first-class choice rather than an empty key.
///
/// [`Credential::Key`] may gain a field in a minor release, so it is built
/// with [`Credential::key`] and a pattern outside this crate ends in `..`:
///
/// ```compile_fail,E0639
/// use abnegate_llm::Credential;
///
/// let credential = Credential::Key {
///     variable: "ANTHROPIC_API_KEY".to_string(),
///     value: "key".into(),
/// };
/// # let _ = credential;
/// ```
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub enum Credential {
    /// The host's existing session. Nothing is injected into the child.
    #[default]
    Inherited,
    /// A key placed in the named environment variable of the child process, or
    /// sent as a bearer token by an HTTP provider.
    #[non_exhaustive]
    Key {
        variable: String,
        value: SecretValue,
    },
}

impl Credential {
    /// The key `value`, placed in the environment variable `variable`.
    pub fn key(variable: impl Into<String>, value: impl Into<SecretValue>) -> Self {
        Self::Key {
            variable: variable.into(),
            value: value.into(),
        }
    }

    /// The credential itself, at the single point where it is used.
    pub fn expose(&self) -> Option<&str> {
        match self {
            Self::Inherited => None,
            Self::Key { value, .. } => Some(value.expose()),
        }
    }

    /// The credential as a [`SecretValue`], for a consumer that holds it
    /// rather than sending it at once.
    pub fn secret(&self) -> Option<&SecretValue> {
        match self {
            Self::Inherited => None,
            Self::Key { value, .. } => Some(value),
        }
    }

    /// The environment variable this credential occupies, if any.
    pub fn variable(&self) -> Option<&str> {
        match self {
            Self::Inherited => None,
            Self::Key { variable, .. } => Some(variable),
        }
    }
}

#[cfg(test)]
mod tests {
    use abnegate_secret::SecretValue;

    use super::Credential;

    #[test]
    fn debug_never_prints_the_credential() {
        let credential =
            Credential::key("ANTHROPIC_API_KEY", concat!("sk-ant-", "api03-notarealkey"));

        let rendered = format!("{credential:?}");
        assert!(
            !rendered.contains("sk-ant"),
            "credential leaked: {rendered}"
        );
        assert!(rendered.contains("[REDACTED]"));
        assert!(rendered.contains("ANTHROPIC_API_KEY"));
    }

    #[test]
    fn expose_is_the_only_reader() {
        let credential = Credential::key("OPENAI_API_KEY", "sk-notarealkey");
        assert_eq!(credential.expose(), Some("sk-notarealkey"));
        assert_eq!(credential.variable(), Some("OPENAI_API_KEY"));
        assert_eq!(
            credential.secret().map(SecretValue::expose),
            Some("sk-notarealkey")
        );
    }

    #[test]
    fn an_inherited_session_injects_nothing() {
        let credential = Credential::default();
        assert!(matches!(credential, Credential::Inherited));
        assert_eq!(credential.expose(), None);
        assert_eq!(credential.variable(), None);
    }
}
