use serde::Deserialize;
use serde::Serialize;

use crate::login::claude::LIFETIME;

const DELIMITER: &str = " ";
const INFERENCE: &[&str] = &["user:inference", "user:profile"];
const FULL: &[&str] = &[
    "org:create_api_key",
    "user:profile",
    "user:inference",
    "user:sessions:claude_code",
    "user:mcp_servers",
    "user:file_upload",
    "user:plugins",
];

/// How much of a Claude account a sign-in asks for, spelled in lowercase on
/// the wire.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum Scope {
    /// Inference and the profile, as `claude setup-token` asks for, with an
    /// access token that lasts a [`LIFETIME`].
    #[default]
    Inference,
    /// Everything the claude CLI's own sign-in asks for, for as long as
    /// claude.com grants.
    Full,
}

impl Scope {
    /// The OAuth scopes this asks for, in the order the claude CLI asks.
    pub fn scopes(self) -> &'static [&'static str] {
        match self {
            Self::Inference => INFERENCE,
            Self::Full => FULL,
        }
    }

    /// The scopes as the `scope` parameter carries them, space-separated.
    pub fn parameter(self) -> String {
        self.scopes().join(DELIMITER)
    }

    pub(crate) fn lifetime(self) -> Option<u64> {
        match self {
            Self::Inference => Some(LIFETIME.as_secs()),
            Self::Full => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_access_asks_for_exactly_what_the_cli_asks_for() {
        assert_eq!(
            Scope::Full.scopes(),
            [
                "org:create_api_key",
                "user:profile",
                "user:inference",
                "user:sessions:claude_code",
                "user:mcp_servers",
                "user:file_upload",
                "user:plugins",
            ]
        );
        assert_eq!(
            Scope::Full.parameter(),
            "org:create_api_key user:profile user:inference user:sessions:claude_code \
             user:mcp_servers user:file_upload user:plugins"
        );
    }

    #[test]
    fn an_inference_sign_in_asks_for_the_profile_too() {
        assert_eq!(
            Scope::Inference.scopes(),
            ["user:inference", "user:profile"]
        );
        assert_eq!(Scope::Inference.parameter(), "user:inference user:profile");
    }

    #[test]
    fn only_an_inference_sign_in_asks_for_a_lifetime() {
        assert_eq!(Scope::Inference.lifetime(), Some(31_536_000));
        assert_eq!(Scope::Full.lifetime(), None);
    }

    #[test]
    fn a_scope_travels_in_lowercase_and_defaults_to_inference() {
        assert_eq!(
            serde_json::to_string(&Scope::Full).expect("serialise"),
            r#""full""#
        );
        assert_eq!(
            serde_json::from_str::<Scope>(r#""inference""#).expect("deserialise"),
            Scope::Inference
        );
        assert_eq!(Scope::default(), Scope::Inference);
    }
}
