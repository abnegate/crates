use crate::pull_request::application::Account;
use crate::pull_request::application::AccountType;
use crate::pull_request::application::ApplicationError;
use crate::pull_request::repository::named;
use serde::Deserialize;

/// The account an installation is on, as it arrives: a user or organisation
/// with a `login` and a `type`, or an enterprise with a `slug` and neither.
#[derive(Debug, Clone, Deserialize)]
pub(super) struct GitHubAccount {
    #[serde(default)]
    pub(super) login: Option<String>,
    #[serde(default)]
    pub(super) slug: Option<String>,
    #[serde(rename = "type", default)]
    pub(super) kind: Option<AccountType>,
}

impl GitHubAccount {
    /// The account this names, of kind `target` when GitHub gave one beside
    /// it, or else of the kind it names itself.
    ///
    /// A login is taken before a slug. Without a kind, a login is
    /// [`AccountType::Unknown`] and a slug is [`AccountType::Enterprise`].
    /// Neither, or one GitHub would never put in a URL, is
    /// [`ApplicationError::Response`].
    pub(super) fn account(self, target: Option<AccountType>) -> Result<Account, ApplicationError> {
        let kind = target.or(self.kind);
        let (login, kind) = match (self.login, self.slug) {
            (Some(login), _) => (login, kind.unwrap_or(AccountType::Unknown)),
            (None, Some(slug)) => (slug, kind.unwrap_or(AccountType::Enterprise)),
            (None, None) => return Err(ApplicationError::Response),
        };
        if !named(&login) {
            return Err(ApplicationError::Response);
        }
        Ok(Account::new(login, kind))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use serde_json::json;

    fn read(row: Value) -> GitHubAccount {
        serde_json::from_value(row).unwrap()
    }

    #[test]
    fn a_login_github_would_never_put_in_a_url_is_unreadable() {
        for login in ["../x", "acme/other", ""] {
            let error = read(json!({"login": login, "type": "User"}))
                .account(None)
                .expect_err("an unsafe login is refused");

            assert!(matches!(error, ApplicationError::Response));
        }
    }

    #[test]
    fn a_slug_github_would_never_put_in_a_url_is_unreadable() {
        let error = read(json!({"slug": "acme/other"}))
            .account(Some(AccountType::Enterprise))
            .expect_err("an unsafe slug is refused");

        assert!(matches!(error, ApplicationError::Response));
    }

    #[test]
    fn an_account_with_neither_login_nor_slug_is_unreadable() {
        let error = read(json!({"id": 1, "type": "User"}))
            .account(Some(AccountType::User))
            .expect_err("an account with no name is refused");

        assert!(matches!(error, ApplicationError::Response));
    }

    #[test]
    fn the_accounts_own_type_is_used_when_no_target_is_given() {
        let account = read(json!({"login": "acme", "type": "Organization"}))
            .account(None)
            .unwrap();

        assert_eq!(account, Account::new("acme", AccountType::Organization));
    }

    #[test]
    fn the_target_is_taken_over_the_accounts_own_type() {
        let account = read(json!({"login": "acme", "type": "User"}))
            .account(Some(AccountType::Organization))
            .unwrap();

        assert_eq!(account.kind, AccountType::Organization);
    }

    #[test]
    fn a_login_with_no_kind_anywhere_is_of_unknown_kind() {
        let account = read(json!({"login": "octocat"})).account(None).unwrap();

        assert_eq!(account, Account::new("octocat", AccountType::Unknown));
    }

    #[test]
    fn a_slug_with_no_kind_anywhere_is_an_enterprise() {
        let account = read(json!({"slug": "acme-enterprise", "name": "Acme Enterprise"}))
            .account(None)
            .unwrap();

        assert_eq!(
            account,
            Account::new("acme-enterprise", AccountType::Enterprise)
        );
    }
}
