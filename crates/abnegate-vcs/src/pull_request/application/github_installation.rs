use crate::pull_request::application::AccountType;
use crate::pull_request::application::ApplicationError;
use crate::pull_request::application::ApplicationId;
use crate::pull_request::application::Installation;
use crate::pull_request::application::InstallationId;
use crate::pull_request::application::RepositorySelection;
use crate::pull_request::application::github_account::GitHubAccount;
use crate::pull_request::application::installation_token::instant;
use serde::Deserialize;

/// One installation of a GitHub App, as it arrives.
#[derive(Debug, Clone, Deserialize)]
pub(super) struct GitHubInstallation {
    pub(super) id: InstallationId,
    pub(super) app_id: ApplicationId,
    #[serde(default)]
    pub(super) account: Option<GitHubAccount>,
    pub(super) target_type: AccountType,
    pub(super) repository_selection: RepositorySelection,
    pub(super) html_url: String,
    #[serde(default)]
    pub(super) suspended_at: Option<String>,
}

impl TryFrom<GitHubInstallation> for Installation {
    type Error = ApplicationError;

    /// The installation `wire` describes, its account of the kind its
    /// `target_type` names.
    ///
    /// No account, an account [`GitHubAccount::account`] refuses, or a
    /// `suspended_at` that names no instant is
    /// [`ApplicationError::Response`].
    fn try_from(wire: GitHubInstallation) -> Result<Self, Self::Error> {
        let account = wire
            .account
            .ok_or(ApplicationError::Response)?
            .account(Some(wire.target_type))?;
        let installation = Installation::new(
            wire.id,
            wire.app_id,
            account,
            wire.repository_selection,
            wire.html_url,
        );
        match wire.suspended_at {
            None => Ok(installation),
            Some(suspended_at) => instant(&suspended_at)
                .map(|suspended_at| installation.with_suspended_at(suspended_at))
                .ok_or(ApplicationError::Response),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pull_request::application::Account;
    use crate::pull_request::application::fixtures::at;
    use serde_json::Value;
    use serde_json::json;
    use std::time::Duration;

    const HTML_URL: &str = "https://github.com/organizations/acme/settings/installations/1";

    fn sample() -> Value {
        json!({
            "id": 1,
            "account": {
                "login": "acme",
                "id": 1,
                "node_id": "MDQ6VXNlcjE=",
                "avatar_url": "https://github.com/images/error/acme_happy.gif",
                "type": "Organization",
                "site_admin": false
            },
            "access_tokens_url": "https://api.github.com/app/installations/1/access_tokens",
            "repositories_url": "https://api.github.com/installation/repositories",
            "html_url": HTML_URL,
            "app_id": 1,
            "target_id": 1,
            "target_type": "Organization",
            "permissions": {"checks": "write", "metadata": "read", "contents": "read"},
            "events": ["push", "pull_request"],
            "single_file_name": "config.yaml",
            "has_multiple_single_files": true,
            "single_file_paths": ["config.yml", ".github/issue_TEMPLATE.md"],
            "repository_selection": "selected",
            "created_at": "2018-02-09T20:51:14Z",
            "updated_at": "2018-02-09T20:51:14Z",
            "app_slug": "github-actions",
            "suspended_at": null,
            "suspended_by": null
        })
    }

    fn installation(row: Value) -> Result<Installation, ApplicationError> {
        let wire: GitHubInstallation =
            serde_json::from_value(row).map_err(|_| ApplicationError::Response)?;
        Installation::try_from(wire)
    }

    fn suspended(at: &str) -> Value {
        let mut row = sample();
        row["suspended_at"] = json!(at);
        row
    }

    #[test]
    fn a_documented_installation_reads_into_an_installation() {
        assert_eq!(
            installation(sample()).unwrap(),
            Installation::new(
                InstallationId::new(1),
                ApplicationId::new(1),
                Account::new("acme", AccountType::Organization),
                RepositorySelection::Selected,
                HTML_URL,
            )
        );
    }

    #[test]
    fn a_suspended_installation_records_when() {
        let installation = installation(suspended("2026-01-15T12:00:00Z")).unwrap();

        assert_eq!(installation.suspended_at, Some(at(1_768_478_400)));
    }

    #[test]
    fn a_suspension_with_an_offset_or_a_fraction_names_the_same_instant() {
        let installation = installation(suspended("2026-01-15T13:00:00.250+01:00")).unwrap();

        assert_eq!(
            installation.suspended_at,
            Some(at(1_768_478_400) + Duration::from_millis(250))
        );
    }

    #[test]
    fn a_suspension_that_names_no_instant_is_unreadable() {
        let error = installation(suspended("soon")).expect_err("soon is not a time");

        assert!(matches!(error, ApplicationError::Response));
    }

    #[test]
    fn an_installation_missing_what_it_needs_is_unreadable() {
        let mut no_account = sample();
        no_account["account"] = Value::Null;
        let mut no_application = sample();
        no_application.as_object_mut().unwrap().remove("app_id");
        let mut no_target = sample();
        no_target.as_object_mut().unwrap().remove("target_type");

        for row in [no_account, no_application, no_target] {
            let error = installation(row).expect_err("an incomplete installation is refused");

            assert!(matches!(error, ApplicationError::Response));
        }
    }

    #[test]
    fn an_enterprise_installation_is_named_by_its_slug() {
        let mut row = sample();
        row["target_type"] = json!("Enterprise");
        row["account"] = json!({
            "id": 1,
            "slug": "acme-enterprise",
            "name": "Acme Enterprise",
            "node_id": "MDEwOkVudGVycHJpc2Ux",
            "html_url": "https://github.com/enterprises/acme-enterprise"
        });

        assert_eq!(
            installation(row).unwrap().account,
            Account::new("acme-enterprise", AccountType::Enterprise)
        );
    }
}
