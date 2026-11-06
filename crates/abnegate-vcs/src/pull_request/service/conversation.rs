use super::*;
use crate::commit_sha::CommitSha;
use crate::pull_request::IssueComment;
use crate::pull_request::IssueReference;
use crate::pull_request::ReviewComment;
use crate::pull_request::ReviewEvent;
use crate::pull_request::ReviewRecord;
use crate::pull_request::comment_request::CommentRequest;
use crate::pull_request::github_identified::GitHubIdentified;
use crate::pull_request::github_issue_comment::GitHubIssueComment;
use crate::pull_request::github_review_comment::GitHubReviewComment;
use crate::pull_request::review_request::ReviewRequest;

/// What an answer about a review comment other than the one asked for is
/// reported as.
const ANOTHER_COMMENT: &str = "GitHub answered with a review comment other than the one asked for";

impl PullRequestService {
    /// Every comment on a pull request's conversation, where review bots leave
    /// their summaries, oldest first.
    ///
    /// Reads no further than the first thousand comments, so on a longer
    /// conversation the newest are the ones left unread. A page longer than
    /// the 16 MiB this crate reads is [`PullRequestError::GitHubApi`], never
    /// a partial or empty list.
    ///
    /// Kept for the callers that already read comments this way. A new caller
    /// should page through [`Self::list_comments`] instead, which reads every
    /// comment and the link and time of each.
    pub async fn fetch_issue_comments(
        &self,
        reference: &PullRequestReference,
        token: &SecretValue,
    ) -> PullRequestResult<Vec<IssueComment>> {
        let number = reference.number().to_string();
        let repository = reference.repository();
        let comments: Vec<GitHubIssueComment> = self
            .get_all(
                &self.repository_endpoint(repository, &["issues", &number, "comments"])?,
                token,
            )
            .await?;

        Ok(comments.into_iter().map(IssueComment::from).collect())
    }

    /// Every review submitted on a pull request, oldest first, with the commit
    /// each was submitted on, where a review bot on a paid plan posts its
    /// score.
    ///
    /// Reads no further than the first thousand reviews. A review GitHub
    /// gives no identifier is [`PullRequestError::GitHubApi`], and one
    /// submitted on a commit this crate does not accept is
    /// [`PullRequestError::Parse`]: never a partial list.
    pub async fn fetch_reviews(
        &self,
        reference: &PullRequestReference,
        token: &SecretValue,
    ) -> PullRequestResult<Vec<ReviewRecord>> {
        let number = reference.number().to_string();
        let repository = reference.repository();
        let reviews: Vec<GitHubReview> = self
            .get_all(
                &self.repository_endpoint(repository, &["pulls", &number, "reviews"])?,
                token,
            )
            .await?;

        reviews.into_iter().map(recorded).collect()
    }

    /// Comment on a pull request's conversation, which is how a review bot is
    /// asked to look again, and return the identifier GitHub gave the comment.
    ///
    /// An error can follow a comment GitHub accepted: an answer that names no
    /// identifier, or one this crate cannot read or that is too long to, is
    /// [`PullRequestError::GitHubApi`] although the comment was posted. A
    /// caller that retries on an error may post it twice, so it should look
    /// for the comment before posting again.
    ///
    /// Kept for the callers that already post comments this way. A new caller
    /// should use [`Self::post_comment`] instead, which returns the comment as
    /// GitHub recorded it.
    pub async fn post_issue_comment(
        &self,
        reference: &PullRequestReference,
        token: &SecretValue,
        body: &str,
    ) -> PullRequestResult<u64> {
        let number = reference.number().to_string();
        let repository = reference.repository();
        let url = self.repository_endpoint(repository, &["issues", &number, "comments"])?;

        let posted: GitHubIdentified = self
            .exchange(
                self.request(Method::POST, url, token, ACCEPT)?
                    .json(&CommentRequest { body }),
            )
            .await?;
        Ok(posted.id)
    }

    /// Comment on an issue's conversation, or a pull request's through
    /// [`IssueReference::from`], and return the comment as GitHub recorded it.
    ///
    /// An error can follow a comment GitHub accepted: an answer this crate
    /// cannot read, or that is too long to, is [`PullRequestError::GitHubApi`]
    /// although the comment was posted. A caller that retries on an error may
    /// post it twice, so it should look for the comment before posting again.
    pub async fn post_comment(
        &self,
        reference: &IssueReference,
        token: &SecretValue,
        body: &str,
    ) -> PullRequestResult<IssueComment> {
        let number = reference.number().to_string();
        let url =
            self.repository_endpoint(reference.repository(), &["issues", &number, "comments"])?;

        let posted: GitHubIssueComment = self
            .exchange(
                self.request(Method::POST, url, token, ACCEPT)?
                    .json(&CommentRequest { body }),
            )
            .await?;
        Ok(IssueComment::from(posted))
    }

    /// Submit a review of the pull request as it stood at `commit`, and return
    /// the identifier GitHub gave the review.
    ///
    /// A token that opened the pull request may only comment: GitHub refuses
    /// it an approval of, or a request for changes to, its own change.
    ///
    /// An error can follow a review GitHub accepted: an answer that names no
    /// identifier, or one this crate cannot read or that is too long to, is
    /// [`PullRequestError::GitHubApi`] although the review was submitted. A
    /// caller that retries on an error may submit it twice, so it should look
    /// for the review before submitting again.
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
        let url = self.repository_endpoint(repository, &["pulls", &number, "reviews"])?;
        let review = ReviewRequest {
            commit_id: commit.as_str(),
            body,
            event,
        };

        let submitted: GitHubIdentified = self
            .exchange(
                self.request(Method::POST, url, token, ACCEPT)?
                    .json(&review),
            )
            .await?;
        Ok(submitted.id)
    }

    /// The review comment GitHub identifies as `comment`, whole, so long as it
    /// was left on the pull request `reference` names.
    ///
    /// A comment GitHub cannot find is [`PullRequestError::NotFound`], and so
    /// is one GitHub places on another pull request, in another repository,
    /// on another host, or on no pull request at all: the pull request it is
    /// placed on has to be addressed at this service's own origin, as
    /// `reference`'s is. A repository that has been renamed or
    /// transferred since `reference` was named is answered for under its new
    /// name, so its comments are [`PullRequestError::NotFound`] too until
    /// `reference` names it as it is now. An answer about a comment other than
    /// `comment` is [`PullRequestError::GitHubApi`].
    pub async fn fetch_review_comment(
        &self,
        reference: &PullRequestReference,
        token: &SecretValue,
        comment: u64,
    ) -> PullRequestResult<ReviewComment> {
        let identifier = comment.to_string();
        let url =
            self.repository_endpoint(reference.repository(), &["pulls", "comments", &identifier])?;

        let answer: GitHubReviewComment = self.get(url, token).await?;
        if answer.id != comment {
            return Err(PullRequestError::GitHubApi(ANOTHER_COMMENT.to_string()));
        }
        if !answer
            .pull_request_url
            .as_deref()
            .is_some_and(|url| self.addresses(url, reference))
        {
            return Err(PullRequestError::NotFound);
        }
        Ok(ReviewComment::from(answer))
    }

    /// Whether `url`, GitHub's REST address of a pull request, is where this
    /// service addresses the pull request `reference` names: on its origin's
    /// scheme, host and port, under its origin's path, and compared without
    /// regard to case, as GitHub compares an owner and a name.
    fn addresses(&self, url: &str, reference: &PullRequestReference) -> bool {
        let Ok(url) = Url::parse(url) else {
            return false;
        };
        let number = reference.number().to_string();
        let Ok(own) = self.repository_endpoint(reference.repository(), &["pulls", &number]) else {
            return false;
        };
        self.origin.holds(&url) && url.path().eq_ignore_ascii_case(own.path())
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
        let url = self.repository_endpoint(
            repository,
            &["pulls", &number, "comments", &comment, "replies"],
        )?;

        self.answered(
            self.request(Method::POST, url, token, ACCEPT)?
                .json(&CommentRequest { body }),
        )
        .await?;
        Ok(())
    }
}

/// A review as GitHub returned it, held to an identifier and to a commit this
/// crate accepts.
fn recorded(review: GitHubReview) -> PullRequestResult<ReviewRecord> {
    let id = review
        .id
        .ok_or_else(|| PullRequestError::GitHubApi(UNREADABLE.to_string()))?;
    Ok(ReviewRecord {
        id,
        author: review.user.and_then(|user| user.login).unwrap_or_default(),
        body: review.body.unwrap_or_default(),
        commit: review
            .commit_id
            .as_deref()
            .map(CommitSha::parse)
            .transpose()?,
        submitted_at: review.submitted_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pull_request::service::fixtures::Expected;
    use crate::pull_request::service::fixtures::commit;
    use crate::pull_request::service::fixtures::project;
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

    const REVIEW_COMMENT_PATH: &str = "/repos/acme/project/pulls/comments/99";

    const PULL_REQUEST_PATH: &str = "/repos/acme/project/pulls/7";

    fn written(id: u64, author: &str, body: &str) -> IssueComment {
        IssueComment {
            id,
            author: author.to_string(),
            body: body.to_string(),
            url: format!("https://github.com/acme/project/pull/7#issuecomment-{id}"),
            created_at: "2026-09-01T12:00:00Z".to_string(),
        }
    }

    fn described(comment: &IssueComment) -> Value {
        json!({
            "id": comment.id,
            "user": { "login": comment.author },
            "body": comment.body,
            "html_url": comment.url,
            "created_at": comment.created_at,
        })
    }

    fn reviewed(id: u64, body: &str) -> ReviewRecord {
        ReviewRecord::new(id, "review-bot", body)
            .with_commit(commit('a'))
            .with_submitted_at("2026-09-28T02:51:09Z")
    }

    fn submitted(review: &ReviewRecord) -> Value {
        json!({
            "id": review.id,
            "user": { "login": review.author },
            "body": review.body,
            "state": "COMMENTED",
            "commit_id": review.commit.as_ref().map(CommitSha::as_str),
            "submitted_at": review.submitted_at,
        })
    }

    #[tokio::test]
    async fn reviews_carry_the_commit_they_were_submitted_on_and_their_body() {
        let server = MockServer::start().await;
        let head = CommitSha::parse("39edddf0d1abc06b84ebb74e8213c1fd41ec333e").unwrap();
        Mock::given(method("GET"))
            .and(path("/repos/acme/project/pulls/7/reviews"))
            .and(header("authorization", "Bearer token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                {
                    "id": 5_333_502_514u64,
                    "user": { "login": "coderabbitai[bot]" },
                    "body": "**Actionable comments posted: 1**",
                    "state": "COMMENTED",
                    "commit_id": head.as_str(),
                    "submitted_at": "2026-09-28T02:51:09Z",
                },
                {
                    "id": 5_333_523_878u64,
                    "user": null,
                    "body": null,
                    "state": "COMMENTED",
                    "commit_id": head.as_str(),
                    "submitted_at": "2026-09-28T02:55:47Z",
                },
            ])))
            .expect(1)
            .mount(&server)
            .await;
        let service = stand_in(&server).await;

        let reviews = service
            .fetch_reviews(&seven(&service), &token())
            .await
            .unwrap();

        assert_eq!(
            reviews,
            [
                ReviewRecord::new(
                    5_333_502_514,
                    "coderabbitai[bot]",
                    "**Actionable comments posted: 1**",
                )
                .with_commit(head.clone())
                .with_submitted_at("2026-09-28T02:51:09Z"),
                ReviewRecord::new(5_333_523_878, "", "")
                    .with_commit(head)
                    .with_submitted_at("2026-09-28T02:55:47Z"),
            ],
            "a review from a deleted account with no body still names its commit"
        );
    }

    #[tokio::test]
    async fn every_page_of_reviews_is_read_in_order() {
        let server = MockServer::start().await;
        let full: Vec<ReviewRecord> = (1..=u64::try_from(PAGE_SIZE).unwrap())
            .map(|id| reviewed(id, &format!("Score {id}")))
            .collect();
        let pending = ReviewRecord::new(101, "review-bot", "Still reading.");
        let reviews_path = "/repos/acme/project/pulls/7/reviews";
        Mock::given(method("GET"))
            .and(path(reviews_path))
            .and(query_param("per_page", "100"))
            .and(query_param("page", "1"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(full.iter().map(submitted).collect::<Vec<Value>>()),
            )
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(reviews_path))
            .and(query_param("per_page", "100"))
            .and(query_param("page", "2"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([{
                "id": pending.id,
                "user": { "login": pending.author },
                "body": pending.body,
                "state": "PENDING",
            }])))
            .expect(1)
            .mount(&server)
            .await;
        let service = stand_in(&server).await;

        let reviews = service
            .fetch_reviews(&seven(&service), &token())
            .await
            .unwrap();

        let expected: Vec<ReviewRecord> = full.into_iter().chain([pending]).collect();
        assert_eq!(reviews, expected);
    }

    #[tokio::test]
    async fn a_review_without_an_identifier_or_on_a_commit_not_accepted_is_an_error() {
        for (answer, expected) in [
            (
                json!([{ "body": SENTINEL, "commit_id": commit('a').as_str() }]),
                (|failure: &PullRequestError| matches!(failure, PullRequestError::GitHubApi(text) if text == UNREADABLE))
                    as Expected,
            ),
            (
                json!([{ "id": 1, "body": SENTINEL, "commit_id": "HEAD" }]),
                |failure: &PullRequestError| matches!(failure, PullRequestError::Parse(_)),
            ),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/repos/acme/project/pulls/7/reviews"))
                .respond_with(ResponseTemplate::new(200).set_body_json(&answer))
                .expect(1)
                .mount(&server)
                .await;
            let service = stand_in(&server).await;

            let failure = service
                .fetch_reviews(&seven(&service), &token())
                .await
                .unwrap_err();

            assert!(expected(&failure), "{answer}: {failure:?}");
            assert!(!format!("{failure:?}").contains(SENTINEL), "{failure:?}");
        }
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
                    .set_body_json(full.iter().map(described).collect::<Vec<Value>>()),
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
            .respond_with(ResponseTemplate::new(201).set_body_json(described(&posted)))
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
    async fn a_comment_is_posted_to_an_issue_or_a_pull_request_and_returned_whole() {
        for (number, posted) in [(5, 1_234_567_890_124), (7, 1_234_567_890_123)] {
            let server = MockServer::start().await;
            let comment = IssueComment::new(
                posted,
                "automation",
                "Looking into it.",
                format!("https://github.com/acme/project/issues/{number}#issuecomment-{posted}"),
                "2026-09-03T00:00:00Z",
            );
            Mock::given(method("POST"))
                .and(path(format!(
                    "/repos/acme/project/issues/{number}/comments"
                )))
                .and(header("authorization", "Bearer token"))
                .and(header("accept", ACCEPT))
                .and(body_json(json!({ "body": "Looking into it." })))
                .respond_with(ResponseTemplate::new(201).set_body_json(described(&comment)))
                .expect(1)
                .mount(&server)
                .await;
            let service = stand_in(&server).await;
            let reference = match number {
                7 => IssueReference::from(&seven(&service)),
                _ => IssueReference::new(project(&service), NonZeroU64::new(number).unwrap()),
            };

            let returned = service
                .post_comment(&reference, &token(), "Looking into it.")
                .await
                .unwrap();

            assert_eq!(returned, comment, "#{number}");
        }
    }

    #[tokio::test]
    async fn a_posted_comment_github_describes_in_part_is_an_error() {
        for answer in [
            json!({ "id": 1, "body": SENTINEL, "created_at": "2026-09-03T00:00:00Z" }),
            json!({ "id": 1, "body": SENTINEL, "html_url": SENTINEL }),
            json!({ "body": SENTINEL, "html_url": SENTINEL, "created_at": SENTINEL }),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/repos/acme/project/issues/7/comments"))
                .respond_with(ResponseTemplate::new(201).set_body_json(&answer))
                .expect(1)
                .mount(&server)
                .await;
            let service = stand_in(&server).await;

            let failure = service
                .post_comment(
                    &IssueReference::from(&seven(&service)),
                    &token(),
                    "Looking into it.",
                )
                .await
                .unwrap_err();

            assert!(
                matches!(failure, PullRequestError::GitHubApi(ref text) if text == UNREADABLE),
                "{answer}: {failure:?}"
            );
            assert!(!format!("{failure:?}").contains(SENTINEL), "{failure:?}");
        }
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
    async fn a_reply_to_a_comment_github_cannot_find_is_not_found() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/repos/acme/project/pulls/7/comments/42/replies"))
            .respond_with(ResponseTemplate::new(404).set_body_json(json!({
                "message": "Not Found",
                "documentation_url": SENTINEL,
            })))
            .expect(1)
            .mount(&server)
            .await;
        let service = stand_in(&server).await;

        let failure = service
            .reply_to_review_comment(&seven(&service), &token(), 42, "Addressed.")
            .await
            .unwrap_err();

        assert!(matches!(failure, PullRequestError::NotFound), "{failure:?}");
    }

    #[tokio::test]
    async fn a_refused_comment_carries_github_s_words_and_not_its_body() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/repos/acme/project/issues/7/comments"))
            .respond_with(ResponseTemplate::new(422).set_body_json(json!({
                "message": "Validation Failed",
                "errors": [{
                    "resource": "IssueComment",
                    "code": "custom",
                    "field": "body",
                    "message": "Body is too long (maximum is 65536 characters)",
                }],
                "documentation_url": SENTINEL,
            })))
            .expect(1)
            .mount(&server)
            .await;
        let service = stand_in(&server).await;

        let failure = service
            .post_issue_comment(&seven(&service), &token(), "@review-bot review")
            .await
            .unwrap_err();

        assert!(
            matches!(
                failure,
                PullRequestError::Refused { status: 422, ref message }
                    if message == "Validation Failed; Body is too long (maximum is 65536 characters)"
            ),
            "{failure:?}"
        );
        assert!(
            !format!("{failure} {failure:?}").contains(SENTINEL),
            "{failure:?}"
        );
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
                PullRequestError::Refused { status: 422, ref message }
                    if message == "Unprocessable Entity; Can not approve your own pull request"
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

    fn remarked() -> ReviewComment {
        ReviewComment::new(
            99,
            "review-bot",
            "handle the empty cart",
            "https://github.com/acme/project/pull/7#discussion_r99",
            "2026-09-20T10:00:00Z",
        )
        .with_path("src/cart.ts")
    }

    fn placed(origin: &str, pull_request_path: &str) -> Value {
        let comment = remarked();
        json!({
            "id": comment.id,
            "user": { "login": comment.author },
            "body": comment.body,
            "path": comment.path,
            "html_url": comment.url,
            "created_at": comment.created_at,
            "pull_request_url": format!("{origin}{pull_request_path}"),
        })
    }

    fn found(answer: Value) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(answer)
    }

    async fn read_answered_with(
        answer: impl FnOnce(&str) -> ResponseTemplate,
    ) -> PullRequestResult<ReviewComment> {
        read_under("", answer).await
    }

    async fn read_under(
        prefix: &str,
        answer: impl FnOnce(&str) -> ResponseTemplate,
    ) -> PullRequestResult<ReviewComment> {
        let server = MockServer::start().await;
        let origin = format!("{}{prefix}", server.uri());
        Mock::given(method("GET"))
            .and(path(format!("{prefix}{REVIEW_COMMENT_PATH}")))
            .and(header("authorization", "Bearer token"))
            .respond_with(answer(&origin))
            .expect(1)
            .mount(&server)
            .await;
        let service = PullRequestService::standing_in_for("github.com", &origin).unwrap();

        service
            .fetch_review_comment(&seven(&service), &token(), 99)
            .await
    }

    #[tokio::test]
    async fn a_review_comment_is_read_whole_by_its_id() {
        let comment = read_answered_with(|origin| found(placed(origin, PULL_REQUEST_PATH)))
            .await
            .unwrap();

        assert_eq!(comment, remarked());
    }

    #[tokio::test]
    async fn a_review_comment_missing_its_author_body_or_file_is_read_as_empty() {
        let comment = read_answered_with(|origin| {
            let mut answer = placed(origin, PULL_REQUEST_PATH);
            answer["user"] = Value::Null;
            answer["body"] = Value::Null;
            answer.as_object_mut().unwrap().remove("path");
            found(answer)
        })
        .await
        .unwrap();

        let expected = remarked();
        assert_eq!(
            comment,
            ReviewComment::new(99, "", "", expected.url, expected.created_at)
        );
    }

    #[tokio::test]
    async fn a_review_comment_github_cannot_find_is_not_found() {
        let failure = read_answered_with(|_| {
            ResponseTemplate::new(404).set_body_json(json!({
                "message": "Not Found",
                "documentation_url": SENTINEL,
            }))
        })
        .await
        .unwrap_err();

        assert!(matches!(failure, PullRequestError::NotFound), "{failure:?}");
    }

    #[tokio::test]
    async fn an_answer_about_another_review_comment_is_an_error() {
        let failure = read_answered_with(|origin| {
            let mut answer = placed(origin, PULL_REQUEST_PATH);
            answer["id"] = json!(100);
            found(answer)
        })
        .await
        .unwrap_err();

        assert!(
            matches!(failure, PullRequestError::GitHubApi(ref text) if text == ANOTHER_COMMENT),
            "{failure:?}"
        );
    }

    #[tokio::test]
    async fn a_review_comment_on_another_pull_request_or_repository_is_not_found() {
        for elsewhere in [
            "/repos/acme/project/pulls/70",
            "/repos/acme/project/pulls/17",
            "/repos/acme/other/pulls/7",
            "/repos/other/project/pulls/7",
        ] {
            let failure = read_answered_with(|origin| found(placed(origin, elsewhere)))
                .await
                .unwrap_err();

            assert!(
                matches!(failure, PullRequestError::NotFound),
                "{elsewhere}: {failure:?}"
            );
        }
    }

    #[tokio::test]
    async fn a_review_comment_on_another_host_is_not_found() {
        let elsewhere: [fn(&str) -> String; 5] = [
            |_| "https://api.github.com".to_string(),
            |_| "https://ghe.example/api/v3".to_string(),
            |origin| origin.replacen("http://", "https://", 1),
            |origin| origin.replacen("127.0.0.1", "localhost", 1),
            |origin| {
                let mut url = Url::parse(origin).unwrap();
                url.set_port(Some(1)).unwrap();
                url.as_str().trim_end_matches('/').to_string()
            },
        ];
        for host in elsewhere {
            let mut placed_at = String::new();
            let failure = read_answered_with(|origin| {
                placed_at = host(origin);
                found(placed(&placed_at, PULL_REQUEST_PATH))
            })
            .await
            .unwrap_err();

            assert!(
                matches!(failure, PullRequestError::NotFound),
                "{placed_at}: {failure:?}"
            );
        }
    }

    #[tokio::test]
    async fn a_review_comment_naming_no_pull_request_is_not_found() {
        let unplaced: [fn(&mut Value); 2] = [
            |answer| {
                answer.as_object_mut().unwrap().remove("pull_request_url");
            },
            |answer| answer["pull_request_url"] = Value::Null,
        ];
        for unplace in unplaced {
            let mut sent = Value::Null;
            let failure = read_answered_with(|origin| {
                sent = placed(origin, PULL_REQUEST_PATH);
                unplace(&mut sent);
                found(sent.clone())
            })
            .await
            .unwrap_err();

            assert!(
                matches!(failure, PullRequestError::NotFound),
                "{sent}: {failure:?}"
            );
        }
    }

    #[tokio::test]
    async fn a_review_comment_belongs_to_its_pull_request_in_any_case() {
        let comment =
            read_answered_with(|origin| found(placed(origin, "/repos/ACME/Project/pulls/7")))
                .await
                .unwrap();

        assert_eq!(comment, remarked());
    }

    #[tokio::test]
    async fn a_review_comment_belongs_to_its_pull_request_under_an_enterprise_prefix() {
        for home in [PULL_REQUEST_PATH, "/repos/ACME/Project/pulls/7"] {
            let comment = read_under("/api/v3", |origin| found(placed(origin, home)))
                .await
                .unwrap();

            assert_eq!(comment, remarked(), "{home}");
        }
    }

    #[tokio::test]
    async fn only_the_pull_request_s_own_address_on_this_origin_belongs() {
        let server = MockServer::start().await;
        let service = stand_in(&server).await;
        let reference = seven(&service);
        let origin = server.uri();

        for stranger in [
            "",
            "not an address",
            "https://github.com/acme/project/pull/7",
            "https://api.github.com/repos/acme/project/pulls/7",
        ] {
            assert!(!service.addresses(stranger, &reference), "{stranger}");
        }
        for stranger in [
            "/repos/acme/project/pulls/7/",
            "/repos/acme/project/pulls/7/files",
            "/repos/acme/project/pulls/07",
            "/repos/acme/project/issues/7",
            "/acme/project/pulls/7",
            "/repos/acme/project-two/pulls/7",
            "/api/v3/repos/acme/project/pulls/7",
        ] {
            let stranger = format!("{origin}{stranger}");
            assert!(!service.addresses(&stranger, &reference), "{stranger}");
        }
        assert!(service.addresses(&format!("{origin}{PULL_REQUEST_PATH}"), &reference));
    }
}
