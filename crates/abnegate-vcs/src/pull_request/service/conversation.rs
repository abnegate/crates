use super::*;
use crate::commit_sha::CommitSha;
use crate::pull_request::IssueComment;
use crate::pull_request::ReviewEvent;
use crate::pull_request::comment_request::CommentRequest;
use crate::pull_request::github_identified::GitHubIdentified;
use crate::pull_request::github_issue_comment::GitHubIssueComment;
use crate::pull_request::review_request::ReviewRequest;

impl PullRequestService {
    /// Every comment on a pull request's conversation, where review bots leave
    /// their summaries, oldest first.
    ///
    /// Reads no further than the first thousand comments, so on a longer
    /// conversation the newest are the ones left unread.
    pub async fn fetch_issue_comments(
        &self,
        reference: &PullRequestReference,
        token: &SecretValue,
    ) -> PullRequestResult<Vec<IssueComment>> {
        let number = reference.number().to_string();
        let repository = reference.repository();
        let comments: Vec<GitHubIssueComment> = self
            .get_all(
                &[
                    "repos",
                    repository.owner(),
                    repository.name(),
                    "issues",
                    &number,
                    "comments",
                ],
                token,
            )
            .await?;

        Ok(comments.into_iter().map(IssueComment::from).collect())
    }

    /// Comment on a pull request's conversation, which is how a review bot is
    /// asked to look again, and return the identifier GitHub gave the comment.
    pub async fn post_issue_comment(
        &self,
        reference: &PullRequestReference,
        token: &SecretValue,
        body: &str,
    ) -> PullRequestResult<u64> {
        let number = reference.number().to_string();
        let repository = reference.repository();
        let url = self.origin.endpoint(&[
            "repos",
            repository.owner(),
            repository.name(),
            "issues",
            &number,
            "comments",
        ]);

        let posted: GitHubIdentified = self
            .exchange(
                self.request(Method::POST, url, token, ACCEPT)
                    .json(&CommentRequest { body }),
            )
            .await?;
        Ok(posted.id)
    }

    /// Submit a review of the pull request as it stood at `commit`, and return
    /// the identifier GitHub gave the review.
    ///
    /// A token that opened the pull request may only comment: GitHub refuses
    /// it an approval of, or a request for changes to, its own change.
    pub async fn submit_review(
        &self,
        reference: &PullRequestReference,
        token: &SecretValue,
        commit: &CommitSha,
        event: ReviewEvent,
        body: &str,
    ) -> PullRequestResult<u64> {
        let number = reference.number().to_string();
        let repository = reference.repository();
        let url = self.origin.endpoint(&[
            "repos",
            repository.owner(),
            repository.name(),
            "pulls",
            &number,
            "reviews",
        ]);
        let review = ReviewRequest {
            commit_id: commit.as_str(),
            body,
            event,
        };

        let submitted: GitHubIdentified = self
            .exchange(self.request(Method::POST, url, token, ACCEPT).json(&review))
            .await?;
        Ok(submitted.id)
    }

    /// Answer the review comment GitHub identifies as `comment`, in its own
    /// thread.
    ///
    /// Once GitHub accepts the reply it is posted, whatever the answer says
    /// about it, so the answer is not read.
    pub async fn reply_to_review_comment(
        &self,
        reference: &PullRequestReference,
        token: &SecretValue,
        comment: u64,
        body: &str,
    ) -> PullRequestResult<()> {
        let number = reference.number().to_string();
        let comment = comment.to_string();
        let repository = reference.repository();
        let url = self.origin.endpoint(&[
            "repos",
            repository.owner(),
            repository.name(),
            "pulls",
            &number,
            "comments",
            &comment,
            "replies",
        ]);

        let response = self
            .request(Method::POST, url, token, ACCEPT)
            .json(&CommentRequest { body })
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(refusal(response).await);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pull_request::service::fixtures::commit;
    use crate::pull_request::service::fixtures::seven;
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
    use wiremock::matchers::query_param;

    const SENTINEL: &str = "RAW-SENTINEL";

    fn written(id: u64, author: &str, body: &str) -> IssueComment {
        IssueComment {
            id,
            author: author.to_string(),
            body: body.to_string(),
            url: format!("https://github.com/acme/project/pull/7#issuecomment-{id}"),
            created_at: "2026-09-01T12:00:00Z".to_string(),
        }
    }

    fn answered(comment: &IssueComment) -> Value {
        json!({
            "id": comment.id,
            "user": { "login": comment.author },
            "body": comment.body,
            "html_url": comment.url,
            "created_at": comment.created_at,
        })
    }

    #[tokio::test]
    async fn a_review_is_submitted_as_a_comment_with_its_commit() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/repos/acme/project/pulls/7/reviews"))
            .and(header("authorization", "Bearer token"))
            .and(body_json(json!({
                "commit_id": commit('a').as_str(),
                "body": "Reviewed at this commit.",
                "event": "COMMENT",
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": 5 })))
            .expect(1)
            .mount(&server)
            .await;
        let service = stand_in(&server).await;

        let review = service
            .submit_review(
                &seven(&service),
                &token(),
                &commit('a'),
                ReviewEvent::Comment,
                "Reviewed at this commit.",
            )
            .await
            .unwrap();

        assert_eq!(review, 5);
    }

    #[tokio::test]
    async fn a_reply_lands_in_the_thread_of_the_comment_it_answers() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/repos/acme/project/pulls/7/comments/42/replies"))
            .and(header("authorization", "Bearer token"))
            .and(body_json(json!({ "body": "Addressed in aaaaaaa." })))
            .respond_with(ResponseTemplate::new(201).set_body_string(SENTINEL))
            .expect(1)
            .mount(&server)
            .await;
        let service = stand_in(&server).await;

        service
            .reply_to_review_comment(&seven(&service), &token(), 42, "Addressed in aaaaaaa.")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn the_conversation_is_read_with_every_comment_s_author_and_link() {
        let server = MockServer::start().await;
        let full: Vec<IssueComment> = (1..=u64::try_from(PAGE_SIZE).unwrap())
            .map(|id| written(id, "review-bot", &format!("Summary {id}")))
            .collect();
        let silent = written(101, "", "");
        let anonymous = written(102, "", "Left by a deleted account.");
        let comments_path = "/repos/acme/project/issues/7/comments";
        Mock::given(method("GET"))
            .and(path(comments_path))
            .and(header("authorization", "Bearer token"))
            .and(query_param("per_page", "100"))
            .and(query_param("page", "1"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(full.iter().map(answered).collect::<Vec<Value>>()),
            )
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(comments_path))
            .and(query_param("per_page", "100"))
            .and(query_param("page", "2"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                {
                    "id": silent.id,
                    "user": { "login": null },
                    "body": null,
                    "html_url": silent.url,
                    "created_at": silent.created_at,
                },
                {
                    "id": anonymous.id,
                    "user": null,
                    "body": anonymous.body,
                    "html_url": anonymous.url,
                    "created_at": anonymous.created_at,
                },
            ])))
            .expect(1)
            .mount(&server)
            .await;
        let service = stand_in(&server).await;

        let comments = service
            .fetch_issue_comments(&seven(&service), &token())
            .await
            .unwrap();

        let expected: Vec<IssueComment> = full.into_iter().chain([silent, anonymous]).collect();
        assert_eq!(comments, expected);
    }

    #[tokio::test]
    async fn a_comment_missing_its_author_or_body_is_read_as_empty() {
        let server = MockServer::start().await;
        let bare = written(3, "", "");
        Mock::given(method("GET"))
            .and(path("/repos/acme/project/issues/7/comments"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([{
                "id": bare.id,
                "html_url": bare.url,
                "created_at": bare.created_at,
            }])))
            .expect(1)
            .mount(&server)
            .await;
        let service = stand_in(&server).await;

        let comments = service
            .fetch_issue_comments(&seven(&service), &token())
            .await
            .unwrap();

        assert_eq!(comments, [bare]);
    }

    #[tokio::test]
    async fn a_comment_is_posted_to_the_conversation_and_its_id_returned() {
        let server = MockServer::start().await;
        let posted = written(1_234_567_890_123, "automation", "@review-bot review");
        Mock::given(method("POST"))
            .and(path("/repos/acme/project/issues/7/comments"))
            .and(header("authorization", "Bearer token"))
            .and(header("accept", ACCEPT))
            .and(body_json(json!({ "body": "@review-bot review" })))
            .respond_with(ResponseTemplate::new(201).set_body_json(answered(&posted)))
            .expect(1)
            .mount(&server)
            .await;
        let service = stand_in(&server).await;

        let id = service
            .post_issue_comment(&seven(&service), &token(), "@review-bot review")
            .await
            .unwrap();

        assert_eq!(id, posted.id);
    }

    #[tokio::test]
    async fn a_posted_comment_without_an_id_is_an_error() {
        for answer in [
            json!({ "body": SENTINEL }),
            json!({ "id": null, "body": SENTINEL }),
            json!({ "id": SENTINEL }),
            json!({ "id": -1, "body": SENTINEL }),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .respond_with(ResponseTemplate::new(201).set_body_json(&answer))
                .expect(2)
                .mount(&server)
                .await;
            let service = stand_in(&server).await;
            let reference = seven(&service);

            let failures = [
                service
                    .post_issue_comment(&reference, &token(), "@review-bot review")
                    .await
                    .unwrap_err(),
                service
                    .submit_review(
                        &reference,
                        &token(),
                        &commit('a'),
                        ReviewEvent::Comment,
                        "Reviewed at this commit.",
                    )
                    .await
                    .unwrap_err(),
            ];

            for failure in failures {
                assert!(
                    matches!(failure, PullRequestError::GitHubApi(ref text) if text == UNREADABLE),
                    "{answer}: {failure:?}"
                );
                assert!(!format!("{failure:?}").contains(SENTINEL), "{failure:?}");
            }
        }
    }

    #[tokio::test]
    async fn a_refused_review_is_reported_as_what_it_is() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/repos/acme/project/pulls/7/reviews"))
            .respond_with(ResponseTemplate::new(422).set_body_json(json!({
                "message": "Unprocessable Entity",
                "errors": ["Can not approve your own pull request"],
                "documentation_url": SENTINEL,
                "status": "422",
            })))
            .expect(1)
            .mount(&server)
            .await;
        let service = stand_in(&server).await;

        let failure = service
            .submit_review(
                &seven(&service),
                &token(),
                &commit('a'),
                ReviewEvent::Approve,
                "Approved.",
            )
            .await
            .unwrap_err();

        assert!(
            matches!(
                failure,
                PullRequestError::GitHubApi(ref text)
                    if text == "GitHub API returned 422 Unprocessable Entity: \
                                Unprocessable Entity; Can not approve your own pull request"
            ),
            "{failure:?}"
        );
        assert!(!format!("{failure:?}").contains(SENTINEL), "{failure:?}");
        assert!(!failure.to_string().contains(SENTINEL), "{failure}");

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(404).set_body_json(json!({
                "message": "Not Found",
                "documentation_url": SENTINEL,
            })))
            .expect(1)
            .mount(&server)
            .await;
        let service = stand_in(&server).await;

        let failure = service
            .submit_review(
                &seven(&service),
                &token(),
                &commit('a'),
                ReviewEvent::Comment,
                "Reviewed at this commit.",
            )
            .await
            .unwrap_err();

        assert!(matches!(failure, PullRequestError::NotFound), "{failure:?}");
    }
}
