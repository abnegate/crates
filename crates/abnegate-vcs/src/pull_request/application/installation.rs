use crate::pull_request::application::Account;
use crate::pull_request::application::ApplicationId;
use crate::pull_request::application::InstallationId;
use crate::pull_request::application::RepositorySelection;
use std::time::SystemTime;

/// One installation of a GitHub App, as GitHub describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Installation {
    /// GitHub's identifier for the installation.
    pub id: InstallationId,
    /// The App that was installed.
    pub application: ApplicationId,
    /// The account it was installed on.
    pub account: Account,
    /// Which of the account's repositories it can reach.
    pub repository_selection: RepositorySelection,
    /// Where the account manages it in a browser.
    pub html_url: String,
    /// When it was suspended, or `None` while it is active.
    pub suspended_at: Option<SystemTime>,
}

impl Installation {
    /// The installation GitHub identifies by `id`, of the App `application`
    /// on `account`, reaching `repository_selection` and managed at
    /// `html_url`, and not suspended.
    pub fn new(
        id: InstallationId,
        application: ApplicationId,
        account: Account,
        repository_selection: RepositorySelection,
        html_url: impl Into<String>,
    ) -> Self {
        Self {
            id,
            application,
            account,
            repository_selection,
            html_url: html_url.into(),
            suspended_at: None,
        }
    }

    /// When it was suspended.
    #[must_use]
    pub fn with_suspended_at(mut self, suspended_at: SystemTime) -> Self {
        self.suspended_at = Some(suspended_at);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pull_request::application::AccountType;
    use crate::pull_request::application::fixtures::at;

    fn installation() -> Installation {
        Installation::new(
            InstallationId::new(67890),
            ApplicationId::new(12345),
            Account::new("acme", AccountType::Organization),
            RepositorySelection::Selected,
            "https://github.com/organizations/acme/settings/installations/67890",
        )
    }

    #[test]
    fn a_new_installation_is_not_suspended() {
        let installation = installation();

        assert_eq!(installation.id, InstallationId::new(67890));
        assert_eq!(installation.application, ApplicationId::new(12345));
        assert_eq!(
            installation.account,
            Account::new("acme", AccountType::Organization)
        );
        assert_eq!(
            installation.repository_selection,
            RepositorySelection::Selected
        );
        assert_eq!(
            installation.html_url,
            "https://github.com/organizations/acme/settings/installations/67890"
        );
        assert_eq!(installation.suspended_at, None);
    }

    #[test]
    fn a_suspended_installation_records_when() {
        let installation = installation().with_suspended_at(at(1_768_478_400));

        assert_eq!(installation.suspended_at, Some(at(1_768_478_400)));
    }
}
