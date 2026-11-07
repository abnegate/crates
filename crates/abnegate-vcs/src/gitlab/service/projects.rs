use super::*;
use crate::gitlab::gitlab_project::GitLabProject;
use crate::remote_branch_name::RemoteBranchName;

impl GitLabService {
    /// The project's default branch, or `None` for a project GitLab names
    /// none for, such as one whose repository is empty.
    ///
    /// A branch name git would refuse under `refs/heads/` is
    /// [`GitLabError::Parse`], never read as something else.
    ///
    /// ```no_run
    /// use abnegate_secret::SecretValue;
    /// use abnegate_vcs::GitLabService;
    /// use abnegate_vcs::Project;
    ///
    /// # async fn read() -> Result<(), Box<dyn std::error::Error>> {
    /// let service = GitLabService::new()?.with_anonymous_reads();
    /// let project = Project::parse("gitlab-org/gitlab")?;
    /// let branch = service
    ///     .get_default_branch(&project, &SecretValue::new(""))
    ///     .await?;
    /// println!("{branch:?}");
    /// # Ok(())
    /// # }
    /// ```
    pub async fn get_default_branch(
        &self,
        project: &Project,
        token: &SecretValue,
    ) -> GitLabResult<Option<RemoteBranchName>> {
        let answer: GitLabProject = self.get(self.project_endpoint(project, &[]), token).await?;
        Ok(answer
            .default_branch
            .as_deref()
            .map(RemoteBranchName::parse)
            .transpose()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gitlab::service::fixtures::API;
    use crate::gitlab::service::fixtures::empty;
    use crate::gitlab::service::fixtures::project;
    use crate::gitlab::service::fixtures::served;
    use crate::gitlab::service::fixtures::token;
    use crate::parse_error::ParseError;
    use serde_json::Value;
    use serde_json::json;
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::header;
    use wiremock::matchers::method;
    use wiremock::matchers::path;

    fn described(default_branch: Value) -> Value {
        json!({
            "id": 123,
            "name": "test-project",
            "default_branch": default_branch,
        })
    }

    async fn serving(route: &str, answer: ResponseTemplate) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(route))
            .respond_with(answer)
            .expect(1)
            .mount(&server)
            .await;
        server
    }

    async fn branch_of(
        server: &MockServer,
        project: &Project,
    ) -> GitLabResult<Option<RemoteBranchName>> {
        served(server).get_default_branch(project, &token()).await
    }

    #[tokio::test]
    async fn the_default_branch_is_read_from_the_project_named_as_one_segment() {
        let server = serving(
            &format!("{API}/projects/test-owner%2Ftest-project"),
            ResponseTemplate::new(200).set_body_json(described(json!("main"))),
        )
        .await;

        let branch = branch_of(&server, &Project::parse("test-owner/test-project").unwrap())
            .await
            .unwrap();

        assert_eq!(branch, Some(RemoteBranchName::parse("main").unwrap()));
    }

    #[tokio::test]
    async fn a_project_named_by_its_id_is_read_by_it() {
        let server = serving(
            &format!("{API}/projects/123"),
            ResponseTemplate::new(200).set_body_json(described(json!("trunk"))),
        )
        .await;

        let branch = branch_of(&server, &Project::parse("123").unwrap())
            .await
            .unwrap();

        assert_eq!(branch, Some(RemoteBranchName::parse("trunk").unwrap()));
    }

    #[tokio::test]
    async fn the_token_travels_as_gitlab_s_private_token_header() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(format!("{API}/projects/acme%2Fproject")))
            .and(header("private-token", "test-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(described(json!("main"))))
            .expect(1)
            .mount(&server)
            .await;

        let branch = served(&server)
            .get_default_branch(&project(), &SecretValue::new("test-token"))
            .await
            .unwrap();

        assert_eq!(branch, Some(RemoteBranchName::parse("main").unwrap()));
    }

    #[tokio::test]
    async fn an_anonymous_read_sends_no_token() {
        let server = serving(
            &format!("{API}/projects/acme%2Fproject"),
            ResponseTemplate::new(200).set_body_json(described(json!("main"))),
        )
        .await;

        let branch = served(&server)
            .with_anonymous_reads()
            .get_default_branch(&project(), &empty())
            .await
            .unwrap();

        assert_eq!(branch, Some(RemoteBranchName::parse("main").unwrap()));
        let received = server.received_requests().await.unwrap();
        assert!(!received[0].headers.contains_key("private-token"));
    }

    #[tokio::test]
    async fn a_project_with_no_default_branch_has_none() {
        let server = serving(
            &format!("{API}/projects/acme%2Fproject"),
            ResponseTemplate::new(200).set_body_json(described(Value::Null)),
        )
        .await;

        assert_eq!(branch_of(&server, &project()).await.unwrap(), None);
    }

    #[tokio::test]
    async fn a_project_needs_neither_its_id_nor_its_name_in_the_answer() {
        let server = serving(
            &format!("{API}/projects/acme%2Fproject"),
            ResponseTemplate::new(200).set_body_json(json!({ "default_branch": "develop" })),
        )
        .await;

        let branch = branch_of(&server, &project()).await.unwrap();

        assert_eq!(branch, Some(RemoteBranchName::parse("develop").unwrap()));
    }

    #[tokio::test]
    async fn a_default_branch_git_would_refuse_is_a_parse_error() {
        let server = serving(
            &format!("{API}/projects/acme%2Fproject"),
            ResponseTemplate::new(200).set_body_json(described(json!("a..b"))),
        )
        .await;

        let failure = branch_of(&server, &project()).await.unwrap_err();

        assert!(
            matches!(&failure, GitLabError::Parse(ParseError::BranchName(name)) if name == "a..b"),
            "{failure:?}"
        );
    }

    #[tokio::test]
    async fn a_missing_hidden_or_forbidden_project_is_reported_as_such() {
        for (status, expected) in [
            (
                404,
                (|failure| matches!(failure, GitLabError::NotFound)) as fn(&GitLabError) -> bool,
            ),
            (401, |failure| {
                matches!(failure, GitLabError::AuthenticationFailed)
            }),
            (403, |failure| matches!(failure, GitLabError::Forbidden)),
        ] {
            let server = serving(
                &format!("{API}/projects/acme%2Fproject"),
                ResponseTemplate::new(status),
            )
            .await;

            let failure = branch_of(&server, &project()).await.unwrap_err();

            assert!(expected(&failure), "{status}: {failure:?}");
        }
    }
}
