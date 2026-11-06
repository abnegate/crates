use crate::pull_request::application::AccountType;

/// The GitHub account a GitHub App is installed on.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Account {
    /// The account's login, or an enterprise's slug, which names it in URLs.
    pub login: String,
    /// What kind of account it is.
    pub kind: AccountType,
}

impl Account {
    /// The account of kind `kind` that `login` names.
    pub fn new(login: impl Into<String>, kind: AccountType) -> Self {
        Self {
            login: login.into(),
            kind,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_account_holds_its_login_and_kind() {
        let account = Account::new("octocat", AccountType::User);

        assert_eq!(account.login, "octocat");
        assert_eq!(account.kind, AccountType::User);
    }
}
