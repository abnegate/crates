use crate::value::SecretValue;

/// `Option<SecretValue>` counterparts to the `Option<String>` methods.
///
/// ```
/// use abnegate_secret::OptionalSecretExtension;
/// use abnegate_secret::SecretValue;
///
/// let token = Some(SecretValue::new("value"));
/// assert_eq!(token.expose_as_deref(), Some("value"));
/// ```
pub trait OptionalSecretExtension {
    /// The exposed credential, as [`Option::as_deref`] would give it.
    fn expose_as_deref(&self) -> Option<&str>;
}

impl OptionalSecretExtension for Option<SecretValue> {
    fn expose_as_deref(&self) -> Option<&str> {
        self.as_ref().map(SecretValue::expose)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expose_as_deref_mirrors_option_as_deref() {
        let present: Option<SecretValue> = Some(SecretValue::new("value"));
        let absent: Option<SecretValue> = None;
        assert_eq!(present.expose_as_deref(), Some("value"));
        assert_eq!(absent.expose_as_deref(), None);
    }
}
