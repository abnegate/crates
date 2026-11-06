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
use crate::pull_request::github_issue_row::GitHubIssueRow;

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
        let mut url = self.repository_endpoint(repository, &["issues"])?;
        url.query_pairs_mut().append_pair("state", state.as_str());
        let rows: Vec<GitHubIssueRow> = self.get(paged(url, page, asked), token).await?;
        let read = rows.len();

        let issues = rows
            .into_iter()
            .filter_map(|row| match row {
                GitHubIssueRow::PullRequest { .. } => None,
                GitHubIssueRow::Issue(issue) => Some(issue.within(repository)),
            })
            .collect();
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
            .get(
                self.repository_endpoint(reference.repository(), &["issues", &number])?,
                token,
            )
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
        let url =
            self.repository_endpoint(reference.repository(), &["issues", &number, "comments"])?;
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

        let url = self.repository_endpoint(repository, &["issues"])?;
        let created: GitHubIssue = self
            .exchange(
                self.request(Method::POST, url, token, ACCEPT)
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
        let url = self.repository_endpoint(reference.repository(), &["issues", &number])?;
        answered(
            self.request(Method::PATCH, url, token, ACCEPT)
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pull_request::Label;
    use crate::pull_request::service::fixtures::Expected;
    use crate::pull_request::service::fixtures::project;
    use crate::pull_request::service::fixtures::query_is;
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

    const SENTINEL: &str = "RAW-SENTINEL";

    const CREATED: &str = "2026-09-01T00:00:00Z";

    const UPDATED: &str = "2026-09-02T00:00:00Z";

    /// What GitHub sends on every issue, without which a row is unreadable.
    const REQUIRED: &[&str] = &[
        "number",
        "title",
        "state",
        "html_url",
        "labels",
        "comments",
        "created_at",
        "updated_at",
    ];

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
        Issue::new(
            numbered(service, number),
            format!("Issue {number}"),
            format!("https://github.com/acme/project/issues/{number}"),
            CREATED,
            UPDATED,
        )
        .with_body(format!("Body {number}"))
        .with_author("reporter")
        .with_assignees(Vec::<String>::new())
    }

    fn described(issue: &Issue) -> Value {
        json!({
            "number": issue.reference.number().get(),
            "title": issue.title,
            "body": issue.body,
            "state": issue.state,
            "html_url": issue.url,
            "user": { "login": issue.author },
            "labels": [],
            "assignees": issue.assignees,
            "comments": issue.comments,
            "created_at": issue.created_at,
            "updated_at": issue.updated_at,
            "closed_at": issue.closed_at,
        })
    }

    fn unreadable(failure: &PullRequestError) -> bool {
        matches!(failure, PullRequestError::GitHubApi(text) if text == UNREADABLE)
    }

    /// `whole` with each thing GitHub always sends on an issue left out or
    /// spoiled in turn, with a pull request marker that is no object, and an
    /// answer that is no issue at all.
    fn damaged(whole: &Value) -> Vec<Value> {
        let mut broken: Vec<Value> = REQUIRED
            .iter()
            .map(|required| {
                let mut row = whole.clone();
                row.as_object_mut().unwrap().remove(*required);
                row
            })
            .collect();
        let mut unnumbered = whole.clone();
        unnumbered["number"] = json!(0);
        let mut unlabelled = whole.clone();
        unlabelled["labels"] = json!([{ "color": SENTINEL }]);
        let mut untitled = whole.clone();
        untitled["title"] = Value::Null;
        let mut marked_by_array = whole.clone();
        marked_by_array["pull_request"] = json!([]);
        let mut marked_by_flag = whole.clone();
        marked_by_flag["pull_request"] = json!(true);
        broken.extend([
            unnumbered,
            unlabelled,
            untitled,
            marked_by_array,
            marked_by_flag,
            json!(SENTINEL),
        ]);
        broken
    }

    /// A comment on issue 5 as GitHub sends it, numbered `id`.
    fn remark(id: u64) -> Value {
        json!({
            "id": id,
            "user": { "login": "reviewer" },
            "body": format!("Comment {id}"),
            "html_url": format!("https://github.com/acme/project/issues/5#issuecomment-{id}"),
            "created_at": CREATED,
        })
    }

    #[tokio::test]
    async fn a_full_page_of_issues_leaves_out_its_pull_request_and_is_followed() {
        let server = MockServer::start().await;
        let service = stand_in(&server).await;
        let issues: Vec<Issue> = (1..=99).map(|number| opened(&service, number)).collect();
        let mut listed: Vec<Value> = issues.iter().map(described).collect();
        let mut pull = described(&opened(&service, 100));
        pull["pull_request"] =
            json!({ "url": "https://api.github.com/repos/acme/project/pulls/100" });
        listed.insert(50, pull);
        Mock::given(method("GET"))
            .and(path("/repos/acme/project/issues"))
            .and(header("authorization", "Bearer token"))
            .and(query_is(&[
                ("state", "open"),
                ("per_page", "100"),
                ("page", "1"),
            ]))
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
        let service = stand_in(&server).await;
        let tiny = opened(&service, 99).with_body("ok");
        let bug = opened(&service, 100).with_body("full issue ".repeat(2000));
        let mut listed = vec![json!({ "number": 1, "pull_request": {} }); 98];
        listed.push(described(&tiny));
        listed.push(described(&bug));
        Mock::given(method("GET"))
            .and(path("/repos/acme/project/issues"))
            .and(query_is(&[
                ("state", "all"),
                ("per_page", "100"),
                ("page", "1"),
            ]))
            .respond_with(ResponseTemplate::new(200).set_body_json(listed))
            .expect(1)
            .mount(&server)
            .await;

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

        assert_eq!(listed, Page::new(vec![tiny, bug], Some(page(2))));
    }

    #[tokio::test]
    async fn a_pull_request_row_is_left_out_before_anything_else_of_it_is_read() {
        let server = MockServer::start().await;
        let service = stand_in(&server).await;
        let kept = opened(&service, 4)
            .with_state(IssueState::Closed)
            .with_closed_at(UPDATED);
        let mut unmarked = described(&kept);
        unmarked["pull_request"] = Value::Null;
        Mock::given(method("GET"))
            .and(path("/repos/acme/project/issues"))
            .and(query_is(&[
                ("state", "open"),
                ("per_page", "2"),
                ("page", "3"),
            ]))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                { "title": 7, "labels": SENTINEL, "pull_request": {} },
                unmarked,
            ])))
            .expect(1)
            .mount(&server)
            .await;

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

        assert_eq!(listed, Page::new(vec![kept], Some(page(4))));
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
                .and(query_is(&[
                    ("state", word),
                    ("per_page", "100"),
                    ("page", "1"),
                ]))
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
    async fn an_issue_row_missing_what_github_always_sends_fails_the_page() {
        let server = MockServer::start().await;
        let service = stand_in(&server).await;
        for row in damaged(&described(&opened(&service, 3).with_body(SENTINEL))) {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/repos/acme/project/issues"))
                .respond_with(
                    ResponseTemplate::new(200)
                        .set_body_json(json!([described(&opened(&service, 1)), row])),
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
    async fn what_github_may_leave_out_of_an_issue_is_read_as_not_said() {
        let server = MockServer::start().await;
        let service = stand_in(&server).await;
        let bare = Issue::new(
            numbered(&service, 6),
            "Untouched",
            "https://github.com/acme/project/issues/6",
            CREATED,
            UPDATED,
        );
        let mut nulled = described(&bare);
        for optional in ["body", "user", "assignees", "closed_at"] {
            nulled[optional] = Value::Null;
        }
        let mut omitted = nulled.clone();
        for optional in ["body", "user", "assignees", "closed_at"] {
            omitted.as_object_mut().unwrap().remove(optional);
        }
        Mock::given(method("GET"))
            .and(path("/repos/acme/project/issues"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([nulled, omitted])))
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

        assert_eq!(listed.items, vec![bare.clone(), bare]);
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
                "Document how to run the widget checks locally",
                "https://github.com/acme/project/issues/5",
                CREATED,
                UPDATED,
            )
            .with_body(body)
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
        );
    }

    #[tokio::test]
    async fn a_pull_request_read_as_an_issue_says_it_is_one() {
        let server = MockServer::start().await;
        let service = stand_in(&server).await;
        let pull = opened(&service, 8).with_pull_request(true);
        let mut answer = described(&pull);
        answer["pull_request"] =
            json!({ "url": "https://api.github.com/repos/acme/project/pulls/8" });
        Mock::given(method("GET"))
            .and(path("/repos/acme/project/issues/8"))
            .respond_with(ResponseTemplate::new(200).set_body_json(answer))
            .expect(1)
            .mount(&server)
            .await;

        let issue = service
            .fetch_issue(&numbered(&service, 8), &token())
            .await
            .unwrap();

        assert_eq!(issue, pull);
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
                .and(query_is(&[("per_page", "2"), ("page", "2")]))
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

    #[tokio::test]
    async fn an_issue_missing_what_github_always_sends_is_unreadable() {
        let server = MockServer::start().await;
        let service = stand_in(&server).await;

        for answer in damaged(&described(&opened(&service, 5).with_body(SENTINEL))) {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/repos/acme/project/issues/5"))
                .respond_with(ResponseTemplate::new(200).set_body_json(&answer))
                .expect(1)
                .mount(&server)
                .await;
            let service = stand_in(&server).await;

            let failure = service
                .fetch_issue(&numbered(&service, 5), &token())
                .await
                .unwrap_err();

            assert!(unreadable(&failure), "{answer}: {failure:?}");
            assert!(!format!("{failure:?}").contains(SENTINEL), "{failure:?}");
        }
    }

    #[tokio::test]
    async fn an_issue_github_cannot_find_is_not_found() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/acme/project/issues/5"))
            .respond_with(ResponseTemplate::new(404).set_body_json(json!({
                "message": "Not Found",
                "documentation_url": SENTINEL,
            })))
            .expect(1)
            .mount(&server)
            .await;
        let service = stand_in(&server).await;

        let failure = service
            .fetch_issue(&numbered(&service, 5), &token())
            .await
            .unwrap_err();

        assert!(matches!(failure, PullRequestError::NotFound), "{failure:?}");
    }

    #[tokio::test]
    async fn a_comment_github_describes_in_part_fails_the_page() {
        let whole = remark(2);
        let mut broken: Vec<Value> = ["id", "html_url", "created_at"]
            .iter()
            .map(|required| {
                let mut row = whole.clone();
                row.as_object_mut().unwrap().remove(*required);
                row
            })
            .collect();
        for (field, spoiled) in [
            ("id", Value::Null),
            ("id", json!(-1)),
            ("id", json!(SENTINEL)),
            ("html_url", Value::Null),
            ("created_at", Value::Null),
        ] {
            let mut row = whole.clone();
            row[field] = spoiled;
            broken.push(row);
        }
        broken.push(json!(SENTINEL));

        for row in broken {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/repos/acme/project/issues/5/comments"))
                .and(query_is(&[("per_page", "2"), ("page", "1")]))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!([remark(1), row])))
                .expect(1)
                .mount(&server)
                .await;
            let service = stand_in(&server).await;

            let failure = service
                .list_comments(&numbered(&service, 5), &token(), first(), rows(2))
                .await
                .unwrap_err();

            assert!(unreadable(&failure), "{row}: {failure:?}");
            assert!(!format!("{failure:?}").contains(SENTINEL), "{failure:?}");
        }
    }

    #[tokio::test]
    async fn a_refused_issue_is_reported_as_what_it_is() {
        let refusals: [(u16, Value, Expected); 3] = [
            (
                410,
                json!({
                    "message": "Issues are disabled for this repo",
                    "documentation_url": SENTINEL,
                }),
                |failure| {
                    matches!(
                        failure,
                        PullRequestError::GitHubApi(text)
                            if text == "GitHub API returned 410 Gone: Issues are disabled for this repo"
                    )
                },
            ),
            (
                422,
                json!({
                    "message": "Validation Failed",
                    "errors": [{
                        "resource": "Issue",
                        "code": "custom",
                        "field": "title",
                        "message": "title is too long (maximum is 256 characters)",
                    }],
                    "documentation_url": SENTINEL,
                }),
                |failure| {
                    matches!(
                        failure,
                        PullRequestError::GitHubApi(text)
                            if text == "GitHub API returned 422 Unprocessable Entity: \
                                        Validation Failed; title is too long (maximum is 256 characters)"
                    )
                },
            ),
            (
                404,
                json!({ "message": "Not Found", "documentation_url": SENTINEL }),
                |failure| matches!(failure, PullRequestError::NotFound),
            ),
        ];

        for (status, answer, expected) in refusals {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/repos/acme/project/issues"))
                .respond_with(ResponseTemplate::new(status).set_body_json(answer))
                .expect(1)
                .mount(&server)
                .await;
            let service = stand_in(&server).await;

            let failure = service
                .create_issue(&project(&service), &token(), "Issue 12", "Body 12")
                .await
                .unwrap_err();

            assert!(expected(&failure), "{status}: {failure:?}");
            assert!(
                !format!("{failure} {failure:?}").contains(SENTINEL),
                "{failure:?}"
            );
        }
    }

    #[tokio::test]
    async fn an_opened_issue_github_describes_in_part_is_unreadable() {
        let server = MockServer::start().await;
        let service = stand_in(&server).await;

        for answer in damaged(&described(&opened(&service, 12).with_body(SENTINEL))) {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/repos/acme/project/issues"))
                .respond_with(ResponseTemplate::new(201).set_body_json(&answer))
                .expect(1)
                .mount(&server)
                .await;
            let service = stand_in(&server).await;

            let failure = service
                .create_issue(&project(&service), &token(), "Issue 12", "Body 12")
                .await
                .unwrap_err();

            assert!(unreadable(&failure), "{answer}: {failure:?}");
            assert!(!format!("{failure:?}").contains(SENTINEL), "{failure:?}");
        }
    }

    #[tokio::test]
    async fn an_update_to_an_issue_github_cannot_find_is_not_found() {
        let server = MockServer::start().await;
        Mock::given(method("PATCH"))
            .and(path("/repos/acme/project/issues/5"))
            .and(body_json(json!({ "title": "Renamed" })))
            .respond_with(ResponseTemplate::new(404).set_body_json(json!({
                "message": "Not Found",
                "documentation_url": SENTINEL,
            })))
            .expect(1)
            .mount(&server)
            .await;
        let service = stand_in(&server).await;

        let failure = service
            .update_issue(
                &numbered(&service, 5),
                &token(),
                &IssueUpdate::default().with_title("Renamed"),
            )
            .await
            .unwrap_err();

        assert!(matches!(failure, PullRequestError::NotFound), "{failure:?}");
    }
}
