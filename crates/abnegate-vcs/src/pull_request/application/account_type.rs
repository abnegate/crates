use serde::Deserialize;
use std::fmt;

/// What kind of GitHub account a GitHub App is installed on, as GitHub names
/// it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "PascalCase")]
#[non_exhaustive]
pub enum AccountType {
    /// A personal account.
    User,
    /// An organisation, shared by its members.
    Organization,
    /// An enterprise, which owns organisations of its own.
    Enterprise,
    /// A kind GitHub gave that this crate does not know yet.
    #[serde(other)]
    Unknown,
}

impl AccountType {
    /// The kind as GitHub spells it, or `Unknown`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "User",
            Self::Organization => "Organization",
            Self::Enterprise => "Enterprise",
            Self::Unknown => "Unknown",
        }
    }
}

impl fmt::Display for AccountType {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn an_account_type_reads_and_prints_as_github_spells_it() {
        for (kind, spelled) in [
            (AccountType::User, "User"),
            (AccountType::Organization, "Organization"),
            (AccountType::Enterprise, "Enterprise"),
            (AccountType::Unknown, "Unknown"),
        ] {
            assert_eq!(
                serde_json::from_value::<AccountType>(json!(spelled)).unwrap(),
                kind
            );
            assert_eq!(kind.as_str(), spelled);
            assert_eq!(kind.to_string(), spelled);
        }
    }

    #[test]
    fn a_kind_github_has_not_named_yet_is_unknown() {
        assert_eq!(
            serde_json::from_value::<AccountType>(json!("Bot")).unwrap(),
            AccountType::Unknown
        );
    }
}
