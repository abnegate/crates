use super::*;

/// What GitHub says when the ref asked to be deleted is not there.
const ABSENT: &str = "Reference does not exist";

impl PullRequestService {
    /// Delete a branch; one already gone is not an error.
    ///
    /// GitHub answers a branch that is gone with a 404, or with a 422 saying
    /// the reference does not exist. It answers a repository or branch the
    /// token cannot see with the same 404, so that also reads as already gone.
    /// Any other refusal, such as one to delete a protected or default branch,
    /// is reported.
    pub async fn delete_branch(
        &self,
        repository: &Repository,
        token: &SecretValue,
        branch: &BranchName,
    ) -> PullRequestResult<()> {
        let reference = branch.reference();
        let segments: Vec<&str> = ["repos", repository.owner(), repository.name(), "git"]
            .into_iter()
            .chain(reference.split('/'))
            .collect();
        let response = self
            .request(
                Method::DELETE,
                self.origin.endpoint(&segments),
                token,
                ACCEPT,
            )
            .send()
            .await?;

        let status = response.status();
        if status.is_success() || status == StatusCode::NOT_FOUND {
            return Ok(());
        }
        if let Some(failure) = classified(status, response.headers()) {
            return Err(failure);
        }

        let refusal = refusal_of(response).await;
        if status == StatusCode::UNPROCESSABLE_ENTITY && refusal.mentions(ABSENT) {
            return Ok(());
        }
        Err(unexpected(status, &refusal))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse_error::ParseError;
    use crate::pull_request::service::fixtures::project;
    use crate::pull_request::service::fixtures::stand_in;
    use crate::pull_request::service::fixtures::token;
    use serde_json::json;
    use std::mem::discriminant;
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::header;
    use wiremock::matchers::method;
    use wiremock::matchers::path;

    const HEADS: &str = "/repos/acme/project/git/refs/heads";

    async fn delete(server: &MockServer, name: &str) -> PullRequestResult<()> {
        let branch = BranchName::parse(name)?;
        let service = stand_in(server).await;
        service
            .delete_branch(&project(&service), &token(), &branch)
            .await
    }

    async fn answering(response: ResponseTemplate) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("DELETE"))
            .and(path(format!("{HEADS}/task/one")))
            .respond_with(response)
            .expect(1)
            .mount(&server)
            .await;
        server
    }

    #[tokio::test]
    async fn a_branch_is_deleted_by_its_ref_one_encoded_segment_at_a_time() {
        for (name, sent) in [
            ("task/one", "task/one"),
            ("feature#x", "feature%23x"),
            ("fix/%2e%2e", "fix/%252e%252e"),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("DELETE"))
                .and(path(format!("{HEADS}/{sent}")))
                .and(header("authorization", "Bearer token"))
                .and(header("accept", ACCEPT))
                .respond_with(ResponseTemplate::new(204))
                .expect(1)
                .mount(&server)
                .await;

            delete(&server, name).await.expect(name);
        }
    }

    #[tokio::test]
    async fn a_branch_already_gone_is_not_an_error() {
        for response in [
            ResponseTemplate::new(404).set_body_json(json!({ "message": "Not Found" })),
            ResponseTemplate::new(404),
            ResponseTemplate::new(422).set_body_json(json!({ "message": ABSENT })),
            ResponseTemplate::new(422).set_body_json(json!({
                "message": "Validation Failed",
                "errors": ["reference does not exist"],
            })),
        ] {
            let server = answering(response).await;

            delete(&server, "task/one").await.unwrap();
        }
    }

    #[tokio::test]
    async fn a_branch_github_refuses_to_delete_is_reported() {
        for (response, expected) in [
            (
                ResponseTemplate::new(422).set_body_json(json!({
                    "message": "Cannot delete this protected branch",
                    "documentation_url": "RAW-SENTINEL",
                })),
                "GitHub API returned 422 Unprocessable Entity: Cannot delete this protected branch",
            ),
            (
                ResponseTemplate::new(422)
                    .set_body_string("<html>Reference does not exist RAW-SENTINEL</html>"),
                "GitHub API returned 422 Unprocessable Entity",
            ),
            (
                ResponseTemplate::new(500),
                "GitHub API returned 500 Internal Server Error",
            ),
        ] {
            let server = answering(response).await;

            let failure = delete(&server, "task/one").await.unwrap_err();

            assert!(
                matches!(failure, PullRequestError::GitHubApi(ref text) if text == expected),
                "{failure:?}"
            );
            assert!(
                !format!("{failure:?}").contains("RAW-SENTINEL"),
                "{failure:?}"
            );
        }
    }

    #[tokio::test]
    async fn a_branch_name_that_could_change_the_request_target_cannot_be_named() {
        let server = MockServer::start().await;

        for name in [
            "a//b",
            "trailing/",
            "/leading",
            "a/../b",
            "..",
            "a/./b",
            "a?b=c",
            "a\tb",
            ".\t.",
            "a/.\t./b",
            "a\nb",
        ] {
            let failure = delete(&server, name).await.unwrap_err();

            assert!(
                matches!(failure, PullRequestError::Parse(ParseError::BranchName(_))),
                "{name:?}: {failure:?}"
            );
        }
        assert!(
            server.received_requests().await.unwrap().is_empty(),
            "nothing reached GitHub"
        );
    }

    #[tokio::test]
    async fn a_refused_deletion_is_reported_as_what_it_is() {
        for (response, expected) in [
            (
                ResponseTemplate::new(401),
                PullRequestError::AuthenticationFailed,
            ),
            (ResponseTemplate::new(403), PullRequestError::Forbidden),
            (
                ResponseTemplate::new(403).insert_header("x-ratelimit-remaining", "0"),
                PullRequestError::RateLimited,
            ),
            (ResponseTemplate::new(429), PullRequestError::RateLimited),
        ] {
            let server = answering(response).await;

            let failure = delete(&server, "task/one").await.unwrap_err();

            assert_eq!(
                discriminant(&failure),
                discriminant(&expected),
                "{expected:?}: {failure:?}"
            );
        }
    }
}
