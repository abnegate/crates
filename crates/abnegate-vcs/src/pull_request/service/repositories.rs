use super::*;
use crate::pull_request::CreatedRepository;
use crate::pull_request::github_created_repository::GitHubCreatedRepository;
use crate::pull_request::github_user::GitHubUser;
use crate::pull_request::repository::named;
use crate::pull_request::repository_request::RepositoryRequest;

/// What GitHub says when the account already holds a repository by the name
/// asked for.
const EXISTS: &str = "already exists";

/// What an account GitHub does not name is reported as.
const NAMELESS: &str = "GitHub did not name the account the token belongs to";

/// What a new repository is reported as when GitHub describes it with an
/// owner or name this crate does not accept.
const UNACCEPTED: &str = "GitHub described the new repository in terms this crate does not accept";

impl PullRequestService {
    /// Create a repository with an initial commit, so the first task has a
    /// default branch to start from.
    ///
    /// `owner` names an organisation to create it under. `None`, an owner
    /// that is only whitespace, or the token's own login, in any case,
    /// creates it under the token's user instead. Telling an organisation
    /// from the token's own login asks GitHub which account the token belongs
    /// to, which a GitHub App installation token cannot answer.
    ///
    /// A name or owner GitHub would not allow is
    /// [`PullRequestError::InvalidRepositoryName`] before anything is sent; a
    /// name the account already holds is
    /// [`PullRequestError::RepositoryExists`]; an organisation the token
    /// cannot see is [`PullRequestError::NotFound`].
    pub async fn create_repository(
        &self,
        token: &SecretValue,
        owner: Option<&str>,
        name: &str,
        description: &str,
        private: bool,
    ) -> PullRequestResult<CreatedRepository> {
        let owner = owner.map(str::trim).filter(|owner| !owner.is_empty());
        if !named(name) || owner.is_some_and(|owner| !named(owner)) {
            return Err(PullRequestError::InvalidRepositoryName);
        }

        let organisation = match owner {
            Some(owner) => {
                let account = login(self.get(self.origin.endpoint(&["user"]), token).await?)?;
                (!owner.eq_ignore_ascii_case(&account)).then_some(owner)
            }
            None => None,
        };
        let url = match organisation {
            Some(organisation) => self.origin.endpoint(&["orgs", organisation, "repos"]),
            None => self.origin.endpoint(&["user", "repos"]),
        };
        let request = RepositoryRequest {
            name,
            description,
            private,
            auto_init: true,
        };

        let response = self
            .request(Method::POST, url, token, ACCEPT)
            .json(&request)
            .send()
            .await?;

        let status = response.status();
        if status.is_success() {
            return created(decode(response).await?);
        }
        if let Some(failure) = classified(status, response.headers()) {
            return Err(failure);
        }

        let refusal = refusal_of(response).await;
        if status == StatusCode::UNPROCESSABLE_ENTITY && refusal.mentions(EXISTS) {
            return Err(PullRequestError::RepositoryExists(name.to_string()));
        }
        Err(unexpected(status, &refusal))
    }
}

/// The login of the account a token belongs to, which GitHub has to name.
fn login(user: GitHubUser) -> PullRequestResult<String> {
    user.login
        .filter(|login| !login.is_empty())
        .ok_or_else(|| PullRequestError::GitHubApi(NAMELESS.to_string()))
}

/// The repository GitHub described creating, held to the rules a repository
/// and a branch name are held to everywhere else.
fn created(answer: GitHubCreatedRepository) -> PullRequestResult<CreatedRepository> {
    let owner = answer.owner.login.unwrap_or_default();
    let repository = Repository::new(&owner, &answer.name)
        .ok_or_else(|| PullRequestError::GitHubApi(UNACCEPTED.to_string()))?;
    Ok(CreatedRepository {
        repository,
        url: answer.html_url,
        clone_url: answer.clone_url,
        default_branch: BranchName::parse(&answer.default_branch)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse_error::ParseError;
    use crate::pull_request::service::fixtures::stand_in;
    use crate::pull_request::service::fixtures::token;
    use serde_json::Value;
    use serde_json::json;
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::body_json;
    use wiremock::matchers::header;
    use wiremock::matchers::method;
    use wiremock::matchers::path;

    fn described(owner: &str, name: &str, branch: &str) -> Value {
        json!({
            "name": name,
            "owner": { "login": owner },
            "html_url": format!("https://github.com/{owner}/{name}"),
            "clone_url": format!("https://github.com/{owner}/{name}.git"),
            "default_branch": branch,
        })
    }

    fn taken() -> ResponseTemplate {
        ResponseTemplate::new(422).set_body_json(json!({
            "message": "Repository creation failed.",
            "errors": [{
                "resource": "Repository",
                "code": "custom",
                "field": "name",
                "message": "name already exists on this account",
            }],
        }))
    }

    async fn signed_in_as(server: &MockServer, answer: Value, times: u64) {
        Mock::given(method("GET"))
            .and(path("/user"))
            .and(header("authorization", "Bearer token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(answer))
            .expect(times)
            .mount(server)
            .await;
    }

    async fn created_under_the_user(answer: Value) -> PullRequestResult<CreatedRepository> {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/user/repos"))
            .respond_with(ResponseTemplate::new(201).set_body_json(answer))
            .expect(1)
            .mount(&server)
            .await;
        stand_in(&server)
            .await
            .create_repository(&token(), None, "shop", "", false)
            .await
    }

    #[tokio::test]
    async fn a_repository_is_created_under_the_organisation_or_the_user() {
        let server = MockServer::start().await;
        signed_in_as(&server, json!({ "login": "ada" }), 2).await;
        Mock::given(method("POST"))
            .and(path("/orgs/acme/repos"))
            .and(header("authorization", "Bearer token"))
            .and(header("accept", ACCEPT))
            .and(body_json(json!({
                "name": "shop",
                "description": "A shop",
                "private": true,
                "auto_init": true,
            })))
            .respond_with(
                ResponseTemplate::new(201).set_body_json(described("acme", "shop", "main")),
            )
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/user/repos"))
            .and(body_json(json!({
                "name": "shop",
                "description": "A shop",
                "private": false,
                "auto_init": true,
            })))
            .respond_with(taken())
            .expect(1)
            .mount(&server)
            .await;
        let service = stand_in(&server).await;

        let created = service
            .create_repository(&token(), Some("acme"), "shop", "A shop", true)
            .await
            .unwrap();
        assert_eq!(created.repository.to_string(), "acme/shop");
        assert_eq!(created.url, "https://github.com/acme/shop");
        assert_eq!(created.clone_url, "https://github.com/acme/shop.git");
        assert_eq!(created.default_branch.as_str(), "main");

        let refused = service
            .create_repository(&token(), Some(" Ada "), "shop", "A shop", false)
            .await
            .unwrap_err();
        assert!(
            matches!(refused, PullRequestError::RepositoryExists(ref name) if name == "shop"),
            "the token's own login, in any case, is the user: {refused:?}"
        );

        let sent = server.received_requests().await.unwrap().len();
        for (owner, name) in [
            (None, "../x"),
            (Some("acme"), "../x"),
            (None, ".."),
            (None, ""),
            (None, "a/b"),
            (None, "shop?private=false"),
            (None, "shop\n"),
            (Some("../x"), "shop"),
            (Some("acme/evil"), "shop"),
            (Some("ac me"), "shop"),
        ] {
            let refused = service
                .create_repository(&token(), owner, name, "", false)
                .await
                .unwrap_err();
            assert!(
                matches!(refused, PullRequestError::InvalidRepositoryName),
                "{owner:?} {name:?}: {refused:?}"
            );
            assert_eq!(refused.to_string(), "Invalid repository owner or name");
        }
        assert_eq!(
            server.received_requests().await.unwrap().len(),
            sent,
            "a name refused here is never sent"
        );
    }

    #[tokio::test]
    async fn a_repository_is_created_under_the_user_without_asking_who_the_user_is() {
        let server = MockServer::start().await;
        signed_in_as(&server, json!({ "login": "ada" }), 0).await;
        Mock::given(method("POST"))
            .and(path("/user/repos"))
            .and(header("authorization", "Bearer token"))
            .and(body_json(json!({
                "name": "notes",
                "description": "",
                "private": true,
                "auto_init": true,
            })))
            .respond_with(
                ResponseTemplate::new(201).set_body_json(described("ada", "notes", "main")),
            )
            .expect(3)
            .mount(&server)
            .await;
        let service = stand_in(&server).await;

        for owner in [None, Some(""), Some(" \t ")] {
            let created = service
                .create_repository(&token(), owner, "notes", "", true)
                .await
                .unwrap();
            assert_eq!(created.repository.to_string(), "ada/notes", "{owner:?}");
        }
    }

    #[tokio::test]
    async fn a_repository_that_already_exists_under_the_user_is_named() {
        let server = MockServer::start().await;
        signed_in_as(&server, json!({ "login": "ada" }), 0).await;
        Mock::given(method("POST"))
            .and(path("/user/repos"))
            .respond_with(taken())
            .expect(1)
            .mount(&server)
            .await;
        let service = stand_in(&server).await;

        let refused = service
            .create_repository(&token(), None, "shop", "A shop", false)
            .await
            .unwrap_err();

        assert!(
            matches!(refused, PullRequestError::RepositoryExists(ref name) if name == "shop"),
            "{refused:?}"
        );
        assert_eq!(
            refused.to_string(),
            "A repository named shop already exists"
        );
    }

    #[tokio::test]
    async fn a_created_repository_reports_its_default_branch_as_a_branch_name() {
        let created = created_under_the_user(described("ada", "shop", "release/next"))
            .await
            .unwrap();
        assert_eq!(
            created.default_branch,
            BranchName::parse("release/next").unwrap()
        );

        for branch in ["a..b", "", "-main"] {
            let refused = created_under_the_user(described("ada", "shop", branch))
                .await
                .unwrap_err();
            assert!(
                matches!(refused, PullRequestError::Parse(ParseError::BranchName(_))),
                "{branch:?}: {refused:?}"
            );
        }

        let mut null = described("ada", "shop", "main");
        null["default_branch"] = Value::Null;
        let mut missing = described("ada", "shop", "main");
        missing.as_object_mut().unwrap().remove("default_branch");
        for answer in [null, missing] {
            let refused = created_under_the_user(answer.clone()).await.unwrap_err();
            assert!(
                matches!(refused, PullRequestError::GitHubApi(ref text) if text == UNREADABLE),
                "a branch GitHub did not name is not taken to be main: {answer}: {refused:?}"
            );
        }
    }

    #[tokio::test]
    async fn an_account_or_repository_github_describes_unreadably_is_an_error() {
        for account in [json!({}), json!({ "login": null }), json!({ "login": "" })] {
            let server = MockServer::start().await;
            signed_in_as(&server, account.clone(), 1).await;
            Mock::given(method("POST"))
                .respond_with(ResponseTemplate::new(201))
                .expect(0)
                .mount(&server)
                .await;
            let service = stand_in(&server).await;

            let refused = service
                .create_repository(&token(), Some("acme"), "shop", "", false)
                .await
                .unwrap_err();

            assert!(
                matches!(refused, PullRequestError::GitHubApi(ref text) if text == NAMELESS),
                "{account}: {refused:?}"
            );
        }

        let mut ownerless = described("ada", "shop", "main");
        ownerless["owner"] = json!({});
        for answer in [
            described("../RAW-SENTINEL", "shop", "main"),
            described("", "shop", "main"),
            described("ada", "RAW SENTINEL", "main"),
            described("ada", "..", "main"),
            ownerless,
        ] {
            let refused = created_under_the_user(answer.clone()).await.unwrap_err();

            assert!(
                matches!(refused, PullRequestError::GitHubApi(ref text) if text == UNACCEPTED),
                "{answer}: {refused:?}"
            );
            assert!(!format!("{refused:?}").contains("SENTINEL"), "{refused:?}");
        }
    }

    /// A refusal on creating is what its status says on its own; a 422 is a
    /// taken name only when GitHub's own words say so, and otherwise carries
    /// those words and nothing else from the answer.
    #[tokio::test]
    async fn a_refused_creation_is_reported_as_what_it_is() {
        for (response, expected) in [
            (ResponseTemplate::new(401), "AuthenticationFailed"),
            (ResponseTemplate::new(403), "Forbidden"),
            (
                ResponseTemplate::new(403).insert_header("x-ratelimit-remaining", "0"),
                "RateLimited",
            ),
            (ResponseTemplate::new(429), "RateLimited"),
            (ResponseTemplate::new(404), "NotFound"),
            (
                ResponseTemplate::new(422).set_body_json(json!({
                    "message": "Validation Failed",
                    "documentation_url": "https://docs.github.com/already exists",
                })),
                "GitHubApi",
            ),
            (
                ResponseTemplate::new(500).set_body_string("name already exists"),
                "GitHubApi",
            ),
        ] {
            let server = MockServer::start().await;
            signed_in_as(&server, json!({ "login": "ada" }), 1).await;
            Mock::given(method("POST"))
                .and(path("/orgs/acme/repos"))
                .respond_with(response)
                .expect(1)
                .mount(&server)
                .await;
            let service = stand_in(&server).await;

            let refused = service
                .create_repository(&token(), Some("acme"), "shop", "", false)
                .await
                .unwrap_err();

            assert!(
                format!("{refused:?}").starts_with(expected),
                "{expected}: {refused:?}"
            );
        }

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/user"))
            .respond_with(ResponseTemplate::new(401))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(201))
            .expect(0)
            .mount(&server)
            .await;
        let service = stand_in(&server).await;
        let refused = service
            .create_repository(&token(), Some("acme"), "shop", "", false)
            .await
            .unwrap_err();
        assert!(
            matches!(refused, PullRequestError::AuthenticationFailed),
            "{refused:?}"
        );

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/user/repos"))
            .respond_with(ResponseTemplate::new(422).set_body_json(json!({
                "message": "Repository creation failed.",
                "errors": [{ "code": "custom", "message": "name is too long (maximum is 100 characters)" }],
                "documentation_url": "RAW-SENTINEL",
            })))
            .expect(1)
            .mount(&server)
            .await;
        let service = stand_in(&server).await;
        let refused = service
            .create_repository(&token(), None, "shop", "", false)
            .await
            .unwrap_err();
        assert_eq!(
            refused.to_string(),
            "GitHub API error: GitHub API returned 422 Unprocessable Entity: \
             Repository creation failed.; name is too long (maximum is 100 characters)"
        );
        assert!(
            !format!("{refused:?}").contains("RAW-SENTINEL"),
            "{refused:?}"
        );
    }
}
