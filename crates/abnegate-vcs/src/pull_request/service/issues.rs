use super::*;
use crate::pull_request::Issue;
use crate::pull_request::IssueComment;
use crate::pull_request::IssueReference;
use crate::pull_request::IssueState;
use crate::pull_request::IssueUpdate;
use crate::pull_request::Page;
use crate::pull_request::StateFilter;
use crate::pull_request::github_issue::GitHubIssue;
use crate::pull_request::github_issue_comment::GitHubIssueComment;
use crate::pull_request::github_issue_request::GitHubIssueRequest;
use serde_json::Value;
use std::num::NonZeroU8;
use std::num::NonZeroU32;

/// The field GitHub sets on an issue row that is really a pull request.
const PULL_REQUEST: &str = "pull_request";

impl PullRequestService {
    /// Page `page` of `repository`'s issues in `state`, newest first, at
    /// `per_page` rows a page up to GitHub's hundred.
    ///
    /// GitHub lists pull requests among issues; they are left out before
    /// anything else of a row is read, so a page may hold fewer issues than
    /// it read rows. Whether another page follows is judged from the rows
    /// GitHub sent, pull requests included, so a page left empty by them can
    /// still be followed by one that is not. Each body is kept in full. A row
    /// that is not a pull request and that this crate cannot read, such as one
    /// naming no number, is [`PullRequestError::GitHubApi`], never a page
    /// without it.
    pub async fn list_issues(
        &self,
        repository: &Repository,
        token: &SecretValue,
        state: StateFilter,
        page: NonZeroU32,
        per_page: NonZeroU8,
    ) -> PullRequestResult<Page<Issue>> {
        let asked = asked(per_page);
        let mut url = self.issues(repository, &[]);
        url.query_pairs_mut().append_pair("state", state.as_str());
        let rows: Vec<Value> = self.get(paged(url, page, asked), token).await?;
        let read = rows.len();

        let issues = rows
            .into_iter()
            .filter(|row| !names_pull_request(row))
            .map(|row| {
                serde_json::from_value::<GitHubIssue>(row)
                    .map(|issue| issue.within(repository))
                    .map_err(|_| PullRequestError::GitHubApi(UNREADABLE.to_string()))
            })
            .collect::<PullRequestResult<Vec<Issue>>>()?;
        Ok(Page::after(issues, page, read, asked))
    }

    /// One issue with its whole body.
    ///
    /// GitHub answers a pull request's number with the pull request read as
    /// an issue, which comes back with [`Issue::pull_request`] set rather
    /// than as an error, so a caller decides what to make of it.
    pub async fn fetch_issue(
        &self,
        reference: &IssueReference,
        token: &SecretValue,
    ) -> PullRequestResult<Issue> {
        let number = reference.number().to_string();
        let issue: GitHubIssue = self
            .get(self.issues(reference.repository(), &[&number]), token)
            .await?;
        Ok(issue.within(reference.repository()))
    }

    /// Page `page` of the comments on an issue's or a pull request's
    /// conversation, oldest first, at `per_page` rows a page up to GitHub's
    /// hundred.
    ///
    /// A comment GitHub gives no identifier, link or time is
    /// [`PullRequestError::GitHubApi`], never a page without it.
    pub async fn list_comments(
        &self,
        reference: &IssueReference,
        token: &SecretValue,
        page: NonZeroU32,
        per_page: NonZeroU8,
    ) -> PullRequestResult<Page<IssueComment>> {
        let asked = asked(per_page);
        let number = reference.number().to_string();
        let url = self.issues(reference.repository(), &[&number, "comments"]);
        let comments: Vec<GitHubIssueComment> = self.get(paged(url, page, asked), token).await?;
        let read = comments.len();

        let comments = comments.into_iter().map(IssueComment::from).collect();
        Ok(Page::after(comments, page, read, asked))
    }

    /// Open an issue on `repository` titled `title` and saying `body`, and
    /// return it as GitHub recorded it.
    ///
    /// An error can follow an issue GitHub opened: an answer this crate cannot
    /// read, or that is too long to, is [`PullRequestError::GitHubApi`]
    /// although the issue exists. A caller that retries on an error may open
    /// it twice, so it should look for the issue before opening it again.
    pub async fn create_issue(
        &self,
        repository: &Repository,
        token: &SecretValue,
        title: &str,
        body: &str,
    ) -> PullRequestResult<Issue> {
        let request = GitHubIssueRequest {
            title: Some(title),
            body: Some(body),
            ..GitHubIssueRequest::default()
        };

        let created: GitHubIssue = self
            .exchange(
                self.request(Method::POST, self.issues(repository, &[]), token, ACCEPT)
                    .json(&request),
            )
            .await?;
        Ok(created.within(repository))
    }

    /// Change what `update` sets on an issue, and nothing else.
    ///
    /// Once GitHub accepts the change it is made, whatever the answer says
    /// about it, so the answer is not read.
    pub async fn update_issue(
        &self,
        reference: &IssueReference,
        token: &SecretValue,
        update: &IssueUpdate,
    ) -> PullRequestResult<()> {
        let number = reference.number().to_string();
        answered(
            self.request(
                Method::PATCH,
                self.issues(reference.repository(), &[&number]),
                token,
                ACCEPT,
            )
            .json(&GitHubIssueRequest::from(update)),
        )
        .await?;
        Ok(())
    }

    /// Close an issue; one already closed stays closed.
    pub async fn close_issue(
        &self,
        reference: &IssueReference,
        token: &SecretValue,
    ) -> PullRequestResult<()> {
        self.update_issue(
            reference,
            token,
            &IssueUpdate::default().with_state(IssueState::Closed),
        )
        .await
    }

    /// The REST endpoint for `repository`'s issues, followed by `rest`.
    pub(super) fn issues(&self, repository: &Repository, rest: &[&str]) -> Url {
        let segments: Vec<&str> = ["repos", repository.owner(), repository.name(), "issues"]
            .into_iter()
            .chain(rest.iter().copied())
            .collect();
        self.origin.endpoint(&segments)
    }
}

/// Whether a row GitHub listed among issues is a pull request.
fn names_pull_request(row: &Value) -> bool {
    row.get(PULL_REQUEST)
        .is_some_and(|marker| !marker.is_null())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pull_request::Label;
    use crate::pull_request::service::fixtures::project;
    use crate::pull_request::service::fixtures::stand_in;
    use crate::pull_request::service::fixtures::token;
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

    fn first() -> NonZeroU32 {
        NonZeroU32::MIN
    }

    fn page(number: u32) -> NonZeroU32 {
        NonZeroU32::new(number).unwrap()
    }

    fn rows(count: u8) -> NonZeroU8 {
        NonZeroU8::new(count).unwrap()
    }

    fn hundred() -> NonZeroU8 {
        rows(u8::try_from(PAGE_SIZE).unwrap())
    }

    fn numbered(service: &PullRequestService, number: u64) -> IssueReference {
        IssueReference::new(project(service), NonZeroU64::new(number).unwrap())
    }

    fn opened(service: &PullRequestService, number: u64) -> Issue {
        Issue::new(numbered(service, number), format!("Issue {number}"))
            .with_body(format!("Body {number}"))
            .with_url(format!("https://github.com/acme/project/issues/{number}"))
            .with_author("reporter")
            .with_created_at("2026-09-01T00:00:00Z")
            .with_updated_at("2026-09-02T00:00:00Z")
    }

    fn described(issue: &Issue) -> Value {
        json!({
            "number": issue.reference.number().get(),
            "title": issue.title,
            "body": issue.body,
            "state": "open",
            "html_url": issue.url,
            "user": { "login": issue.author },
            "labels": [],
            "assignees": [],
            "comments": issue.comments,
            "created_at": issue.created_at,
            "updated_at": issue.updated_at,
            "closed_at": null,
        })
    }

    fn unreadable(failure: &PullRequestError) -> bool {
        matches!(failure, PullRequestError::GitHubApi(text) if text == UNREADABLE)
    }

    #[tokio::test]
    async fn a_full_page_of_issues_leaves_out_its_pull_request_and_is_followed() {
        let server = MockServer::start().await;
        let service = stand_in(&server).await;
        let issues: Vec<Issue> = (1..=99).map(|number| opened(&service, number)).collect();
        let mut listed: Vec<Value> = issues.iter().map(described).collect();
        listed.insert(
            50,
            json!({
                "number": 100,
                "title": "A pull request",
                "html_url": "https://github.com/acme/project/pull/100",
                "pull_request": { "url": "https://api.github.com/repos/acme/project/pulls/100" },
            }),
        );
        Mock::given(method("GET"))
            .and(path("/repos/acme/project/issues"))
            .and(header("authorization", "Bearer token"))
            .and(query_param("state", "open"))
            .and(query_param("per_page", "100"))
            .and(query_param("page", "1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(listed))
            .expect(1)
            .mount(&server)
            .await;

        let listed = service
            .list_issues(
                &project(&service),
                &token(),
                StateFilter::Open,
                first(),
                hundred(),
            )
            .await
            .unwrap();

        assert_eq!(listed, Page::new(issues, Some(page(2))));
    }

    #[tokio::test]
    async fn pull_request_rows_with_no_issue_fields_are_left_out_and_bodies_kept_whole() {
        let server = MockServer::start().await;
        let body = "full issue ".repeat(2000);
        let mut listed = vec![json!({ "number": 1, "pull_request": {} }); 98];
        listed.push(json!({ "number": 99, "title": "Tiny", "body": "ok" }));
        listed.push(json!({ "number": 100, "title": "Bug", "body": body }));
        Mock::given(method("GET"))
            .and(path("/repos/acme/project/issues"))
            .and(query_param("state", "all"))
            .and(query_param("page", "1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(listed))
            .expect(1)
            .mount(&server)
            .await;
        let service = stand_in(&server).await;

        let listed = service
            .list_issues(
                &project(&service),
                &token(),
                StateFilter::All,
                first(),
                hundred(),
            )
            .await
            .unwrap();

        assert_eq!(
            listed,
            Page::new(
                vec![
                    Issue::new(numbered(&service, 99), "Tiny")
                        .with_body("ok")
                        .with_state(IssueState::Unknown),
                    Issue::new(numbered(&service, 100), "Bug")
                        .with_body(body)
                        .with_state(IssueState::Unknown),
                ],
                Some(page(2)),
            )
        );
    }

    #[tokio::test]
    async fn a_pull_request_row_is_left_out_before_anything_else_of_it_is_read() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/acme/project/issues"))
            .and(query_param("per_page", "2"))
            .and(query_param("page", "3"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                { "title": 7, "labels": SENTINEL, "pull_request": {} },
                { "number": 4, "title": "Kept", "state": "closed", "pull_request": null },
            ])))
            .expect(1)
            .mount(&server)
            .await;
        let service = stand_in(&server).await;

        let listed = service
            .list_issues(
                &project(&service),
                &token(),
                StateFilter::Open,
                page(3),
                rows(2),
            )
            .await
            .unwrap();

        assert_eq!(
            listed,
            Page::new(
                vec![Issue::new(numbered(&service, 4), "Kept").with_state(IssueState::Closed)],
                Some(page(4)),
            )
        );
    }

    #[tokio::test]
    async fn the_list_asks_for_the_state_it_was_given_and_never_more_than_a_hundred_rows() {
        for (filter, word) in [
            (StateFilter::Open, "open"),
            (StateFilter::Closed, "closed"),
            (StateFilter::All, "all"),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/repos/acme/project/issues"))
                .and(query_param("state", word))
                .and(query_param("per_page", "100"))
                .and(query_param("page", "1"))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
                .expect(1)
                .mount(&server)
                .await;
            let service = stand_in(&server).await;

            let listed = service
                .list_issues(&project(&service), &token(), filter, first(), rows(u8::MAX))
                .await
                .unwrap();

            assert_eq!(listed, Page::new(Vec::new(), None), "{word}");
        }
    }

    #[tokio::test]
    async fn an_issue_row_without_a_number_is_an_error_and_never_a_shorter_page() {
        for row in [
            json!({ "title": SENTINEL }),
            json!({ "number": 0, "title": SENTINEL }),
            json!({ "number": 3, "labels": [{ "color": SENTINEL }] }),
            json!(SENTINEL),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/repos/acme/project/issues"))
                .respond_with(
                    ResponseTemplate::new(200)
                        .set_body_json(json!([{ "number": 1, "title": "Fine" }, row])),
                )
                .expect(1)
                .mount(&server)
                .await;
            let service = stand_in(&server).await;

            let failure = service
                .list_issues(
                    &project(&service),
                    &token(),
                    StateFilter::Open,
                    first(),
                    hundred(),
                )
                .await
                .unwrap_err();

            assert!(unreadable(&failure), "{row}: {failure:?}");
            assert!(!format!("{failure:?}").contains(SENTINEL), "{failure:?}");
        }
    }

    #[tokio::test]
    async fn one_issue_is_read_in_full_with_its_labels_and_assignees() {
        let server = MockServer::start().await;
        let body = "x".repeat(5_000);
        Mock::given(method("GET"))
            .and(path("/repos/acme/project/issues/5"))
            .and(header("authorization", "Bearer token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "number": 5,
                "title": "Document how to run the widget checks locally",
                "body": body,
                "state": "open",
                "html_url": "https://github.com/acme/project/issues/5",
                "user": { "login": "reporter" },
                "labels": [
                    { "name": "docs" },
                    "triage",
                    {
                        "name": "good first issue",
                        "color": "7057ff",
                        "description": "Good for newcomers",
                    },
                ],
                "assignees": [{ "login": "octocat" }, { "login": null }, {}],
                "comments": 1,
                "created_at": "2026-09-01T00:00:00Z",
                "updated_at": "2026-09-02T00:00:00Z",
                "closed_at": null,
            })))
            .expect(1)
            .mount(&server)
            .await;
        let service = stand_in(&server).await;

        let issue = service
            .fetch_issue(&numbered(&service, 5), &token())
            .await
            .unwrap();

        assert_eq!(
            issue,
            Issue::new(
                numbered(&service, 5),
                "Document how to run the widget checks locally"
            )
            .with_body(body)
            .with_url("https://github.com/acme/project/issues/5")
            .with_author("reporter")
            .with_labels([
                Label::new("docs"),
                Label::new("triage"),
                Label::new("good first issue")
                    .with_color("7057ff")
                    .with_description("Good for newcomers"),
            ])
            .with_assignees(["octocat"])
            .with_comments(1)
            .with_created_at("2026-09-01T00:00:00Z")
            .with_updated_at("2026-09-02T00:00:00Z")
        );
    }

    #[tokio::test]
    async fn a_pull_request_read_as_an_issue_says_it_is_one() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/acme/project/issues/8"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({ "number": 8, "pull_request": {} })),
            )
            .expect(1)
            .mount(&server)
            .await;
        let service = stand_in(&server).await;

        let issue = service
            .fetch_issue(&numbered(&service, 8), &token())
            .await
            .unwrap();

        assert_eq!(
            issue,
            Issue::new(numbered(&service, 8), "")
                .with_state(IssueState::Unknown)
                .with_pull_request(true)
        );
    }

    #[tokio::test]
    async fn a_page_of_comments_is_read_and_followed_only_when_full() {
        for (served, next) in [(2, Some(page(3))), (1, None)] {
            let server = MockServer::start().await;
            let comments: Vec<IssueComment> = (1..=served)
                .map(|id| {
                    IssueComment::new(
                        id,
                        "reviewer",
                        format!("Comment {id}"),
                        format!("https://github.com/acme/project/issues/5#issuecomment-{id}"),
                        "2026-09-02T00:00:00Z",
                    )
                })
                .collect();
            Mock::given(method("GET"))
                .and(path("/repos/acme/project/issues/5/comments"))
                .and(header("authorization", "Bearer token"))
                .and(query_param("per_page", "2"))
                .and(query_param("page", "2"))
                .respond_with(
                    ResponseTemplate::new(200).set_body_json(
                        comments
                            .iter()
                            .map(|comment| {
                                json!({
                                    "id": comment.id,
                                    "user": { "login": comment.author },
                                    "body": comment.body,
                                    "html_url": comment.url,
                                    "created_at": comment.created_at,
                                })
                            })
                            .collect::<Vec<Value>>(),
                    ),
                )
                .expect(1)
                .mount(&server)
                .await;
            let service = stand_in(&server).await;

            let listed = service
                .list_comments(&numbered(&service, 5), &token(), page(2), rows(2))
                .await
                .unwrap();

            assert_eq!(listed, Page::new(comments, next), "{served} served");
        }
    }

    #[tokio::test]
    async fn an_issue_is_opened_with_its_title_and_body_alone() {
        let server = MockServer::start().await;
        let service = stand_in(&server).await;
        let created = opened(&service, 12);
        Mock::given(method("POST"))
            .and(path("/repos/acme/project/issues"))
            .and(header("authorization", "Bearer token"))
            .and(header("accept", ACCEPT))
            .and(body_json(json!({ "title": "Issue 12", "body": "Body 12" })))
            .respond_with(ResponseTemplate::new(201).set_body_json(described(&created)))
            .expect(1)
            .mount(&server)
            .await;

        let issue = service
            .create_issue(&project(&service), &token(), "Issue 12", "Body 12")
            .await
            .unwrap();

        assert_eq!(issue, created);
    }

    #[tokio::test]
    async fn an_update_sends_exactly_what_was_set() {
        for (update, sent) in [
            (
                IssueUpdate::default().with_title("Renamed"),
                json!({ "title": "Renamed" }),
            ),
            (
                IssueUpdate::default()
                    .with_body("Rewritten.")
                    .with_state(IssueState::Open)
                    .with_labels(["bug"]),
                json!({ "body": "Rewritten.", "state": "open", "labels": ["bug"] }),
            ),
            (
                IssueUpdate::default()
                    .with_title("Renamed")
                    .with_state(IssueState::Unknown),
                json!({ "title": "Renamed" }),
            ),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("PATCH"))
                .and(path("/repos/acme/project/issues/5"))
                .and(header("authorization", "Bearer token"))
                .and(body_json(&sent))
                .respond_with(ResponseTemplate::new(200).set_body_string(SENTINEL))
                .expect(1)
                .mount(&server)
                .await;
            let service = stand_in(&server).await;

            service
                .update_issue(&numbered(&service, 5), &token(), &update)
                .await
                .unwrap();
        }
    }

    #[tokio::test]
    async fn closing_an_issue_sends_only_its_new_state() {
        let server = MockServer::start().await;
        Mock::given(method("PATCH"))
            .and(path("/repos/acme/project/issues/5"))
            .and(header("authorization", "Bearer token"))
            .and(body_json(json!({ "state": "closed" })))
            .respond_with(ResponseTemplate::new(200).set_body_string(SENTINEL))
            .expect(1)
            .mount(&server)
            .await;
        let service = stand_in(&server).await;

        service
            .close_issue(&numbered(&service, 5), &token())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn a_refused_update_is_reported_as_what_it_is() {
        let server = MockServer::start().await;
        Mock::given(method("PATCH"))
            .and(path("/repos/acme/project/issues/5"))
            .respond_with(ResponseTemplate::new(410).set_body_json(json!({
                "message": "Issues are disabled for this repo",
                "documentation_url": SENTINEL,
            })))
            .expect(1)
            .mount(&server)
            .await;
        let service = stand_in(&server).await;

        let failure = service
            .close_issue(&numbered(&service, 5), &token())
            .await
            .unwrap_err();

        assert!(
            matches!(
                failure,
                PullRequestError::GitHubApi(ref text)
                    if text == "GitHub API returned 410 Gone: Issues are disabled for this repo"
            ),
            "{failure:?}"
        );
        assert!(!format!("{failure:?}").contains(SENTINEL), "{failure:?}");
    }
}
