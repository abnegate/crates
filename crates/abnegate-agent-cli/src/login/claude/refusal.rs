const ACCOUNT_ON_HOLD: &str = "account_on_hold";
const INVALID_SCOPE: &str = "invalid_scope";
const SERVER_ERROR: &str = "server_error";
const TEMPORARILY_UNAVAILABLE: &str = "temporarily_unavailable";

/// Why claude.com did not approve a sign-in, as far as it matters for what
/// to do next.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Refusal {
    /// The person declined, or claude.com refused for a reason starting
    /// again can fix.
    Declined,
    /// claude.com would not grant the access the sign-in asked for.
    Scope,
    /// The Claude account is on hold, so no sign-in succeeds until that is
    /// resolved.
    OnHold,
    /// claude.com could not answer just now.
    Unavailable,
}

impl Refusal {
    /// The refusal an OAuth `error` and its `error_description` name.
    /// claude.com marks an account on hold in the description, as the
    /// claude CLI reads it; any other description changes nothing.
    pub fn named(error: &str, description: Option<&str>) -> Self {
        if description == Some(ACCOUNT_ON_HOLD) {
            return Self::OnHold;
        }
        match error {
            INVALID_SCOPE => Self::Scope,
            SERVER_ERROR | TEMPORARILY_UNAVAILABLE => Self::Unavailable,
            _ => Self::Declined,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_error_names_the_refusal_that_says_what_to_do_next() {
        for (error, description, refusal) in [
            ("access_denied", None, Refusal::Declined),
            ("invalid_request", Some("anything"), Refusal::Declined),
            ("invalid_scope", None, Refusal::Scope),
            ("server_error", None, Refusal::Unavailable),
            ("temporarily_unavailable", None, Refusal::Unavailable),
            ("access_denied", Some("account_on_hold"), Refusal::OnHold),
            ("server_error", Some("account_on_hold"), Refusal::OnHold),
        ] {
            assert_eq!(
                Refusal::named(error, description),
                refusal,
                "{error} {description:?}"
            );
        }
    }
}
