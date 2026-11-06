use crate::branch_name::BranchName;
use crate::pull_request::Repository;
use crate::pull_request::application::ApplicationError;
use crate::pull_request::application::InstalledRepository;
use crate::pull_request::github_user::GitHubUser;
use serde::Deserialize;

/// One repository an installation can reach, as GitHub lists it. Its
/// `full_name` is never read: it is derived from the owner and name once
/// both are validated.
#[derive(Debug, Clone, Deserialize)]
pub(super) struct GitHubInstalledRepository {
    pub(super) id: u64,
    pub(super) name: String,
    pub(super) owner: GitHubUser,
    pub(super) private: bool,
    pub(super) archived: bool,
    pub(super) default_branch: String,
    pub(super) html_url: String,
}

impl TryFrom<GitHubInstalledRepository> for InstalledRepository {
    type Error = ApplicationError;

    /// The repository GitHub listed, or [`ApplicationError::Response`] when
    /// its owner has no login, its owner or name is not one GitHub allows,
    /// or its default branch is not a branch git would accept.
    fn try_from(repository: GitHubInstalledRepository) -> Result<Self, Self::Error> {
        let owner = repository.owner.login.ok_or(ApplicationError::Response)?;
        let named =
            Repository::new(&owner, &repository.name).map_err(|_| ApplicationError::Response)?;
        let default_branch = BranchName::parse(&repository.default_branch)
            .map_err(|_| ApplicationError::Response)?;
        Ok(
            Self::new(repository.id, &named, default_branch, repository.html_url)
                .with_private(repository.private)
                .with_archived(repository.archived),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use serde_json::json;

    fn row() -> Value {
        json!({
            "id": 1296269,
            "node_id": "MDEwOlJlcG9zaXRvcnkxMjk2MjY5",
            "name": "project",
            "full_name": "someone/else",
            "owner": {
                "login": "acme",
                "id": 1,
                "type": "Organization",
                "site_admin": false
            },
            "private": true,
            "archived": false,
            "disabled": false,
            "fork": false,
            "html_url": "https://github.com/acme/project",
            "clone_url": "https://github.com/acme/project.git",
            "default_branch": "main",
            "visibility": "private",
            "permissions": { "admin": false, "push": true, "pull": true },
            "pushed_at": "2011-01-26T19:06:43Z"
        })
    }

    fn installed(row: Value) -> Result<InstalledRepository, ApplicationError> {
        let repository: GitHubInstalledRepository =
            serde_json::from_value(row).map_err(|_| ApplicationError::Response)?;
        InstalledRepository::try_from(repository)
    }

    fn with(field: &str, value: Value) -> Value {
        let mut row = row();
        row[field] = value;
        row
    }

    fn without(field: &str) -> Value {
        let mut row = row();
        row.as_object_mut().unwrap().remove(field);
        row
    }

    fn assert_refused(row: Value, case: &str) {
        assert!(
            matches!(installed(row), Err(ApplicationError::Response)),
            "{case} was not refused as Response"
        );
    }

    #[test]
    fn a_listed_repository_with_extra_fields_becomes_an_installed_repository() {
        let repository = installed(row()).unwrap();

        let expected = InstalledRepository::new(
            1296269,
            &Repository::new("acme", "project").unwrap(),
            BranchName::parse("main").unwrap(),
            "https://github.com/acme/project",
        )
        .with_private(true);
        assert_eq!(repository, expected);
        assert_eq!(repository.full_name, "acme/project");
        assert!(repository.private);
        assert!(!repository.archived);
    }

    #[test]
    fn an_archived_repository_stays_archived() {
        let repository = installed(with("archived", json!(true))).unwrap();

        assert!(repository.archived);
    }

    #[test]
    fn an_owner_login_github_does_not_allow_is_refused() {
        for login in ["acme/other", "..", "pro ject"] {
            assert_refused(with("owner", json!({ "login": login })), login);
        }
    }

    #[test]
    fn a_name_github_does_not_allow_is_refused() {
        assert_refused(with("name", json!("../x")), "../x");
    }

    #[test]
    fn an_owner_without_a_login_is_refused() {
        assert_refused(with("owner", json!({ "id": 1 })), "a missing login");
        assert_refused(with("owner", json!({ "login": null })), "a null login");
    }

    #[test]
    fn a_default_branch_git_would_refuse_is_refused() {
        for branch in ["-x", "HEAD"] {
            assert_refused(with("default_branch", json!(branch)), branch);
        }
    }

    #[test]
    fn a_row_missing_its_visibility_flags_is_refused() {
        for field in ["archived", "private"] {
            assert_refused(without(field), field);
        }
    }
}
