use super::*;
use crate::pull_request::service::fixtures::enterprise;
use crate::pull_request::service::fixtures::project;
use crate::pull_request::service::fixtures::stand_in;
use crate::pull_request::service::fixtures::token;
use std::num::NonZeroU8;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::Request;
use wiremock::ResponseTemplate;
use wiremock::matchers::body_json;
use wiremock::matchers::header;
use wiremock::matchers::method;
use wiremock::matchers::path;

const REPOSITORY_ID: &str = "R_kgDOAcmeProject";

const FORK_ID: &str = "R_kgDOForkProject";

const UPDATED: &str = "2026-11-06T09:30:00Z";

const SECONDARY_LIMIT: &str =
    "You have exceeded a secondary rate limit. Please wait a few minutes before you try again.";

const GRAPHQL_TIMEOUT: &str = "Something went wrong while executing your query. This may be the \
result of a timeout, or it could be a GitHub bug.";

fn head() -> String {
    "a".repeat(40)
}

fn identifier(number: u64) -> String {
    format!("PR_{number}")
}

fn identifiers(numbers: impl IntoIterator<Item = u64>) -> Vec<String> {
    numbers.into_iter().map(identifier).collect()
}

fn number_of(id: &str) -> u64 {
    id.trim_start_matches("PR_").parse().unwrap()
}

fn row(number: u64) -> Value {
    json!({
        "id": identifier(number),
        "number": number,
        "title": format!("change {number}"),
        "url": format!("https://github.com/acme/project/pull/{number}"),
        "state": "OPEN",
        "isDraft": false,
        "updatedAt": UPDATED,
        "headRefName": format!("task/{number}"),
        "headRefOid": head(),
        "baseRefName": "main",
        "author": { "__typename": "User", "login": "octocat" },
    })
}

fn check_run(id: u64, name: &str, status: &str, conclusion: Option<&str>) -> Value {
    json!({
        "databaseId": id,
        "name": name,
        "status": status,
        "conclusion": conclusion,
        "url": format!("https://github.com/acme/project/runs/{id}"),
        "detailsUrl": format!("https://github.com/acme/project/actions/runs/500/job/{id}"),
        "startedAt": "2026-11-06T09:00:00Z",
        "completedAt": "2026-11-06T09:05:00Z",
    })
}

fn actions_run(id: u64, workflow: u64) -> Value {
    json!({
        "databaseId": id,
        "event": "pull_request",
        "url": format!("https://github.com/acme/project/actions/runs/{id}"),
        "createdAt": "2026-11-06T09:00:00Z",
        "updatedAt": "2026-11-06T09:06:00Z",
        "workflow": { "databaseId": workflow, "name": "CI" },
        "file": { "path": ".github/workflows/ci.yml" },
    })
}

fn suite(repository: &str, run: Value, check_runs: Value) -> Value {
    let app = match run.is_null() {
        true => "review-app",
        false => "github-actions",
    };
    json!({
        "app": { "slug": app },
        "status": "COMPLETED",
        "conclusion": "SUCCESS",
        "createdAt": "2026-11-06T09:00:00Z",
        "branch": { "name": "task/7" },
        "repository": { "id": repository },
        "workflowRun": run,
        "checkRuns": { "pageInfo": { "hasNextPage": false }, "nodes": check_runs },
    })
}

fn comment(id: &str, login: &str, kind: &str) -> Value {
    json!({
        "fullDatabaseId": id,
        "body": "looks good",
        "url": format!("https://github.com/acme/project/pull/7#issuecomment-{id}"),
        "createdAt": "2026-11-06T09:10:00Z",
        "author": { "__typename": kind, "login": login },
    })
}

fn evidence_node(number: u64) -> Value {
    let mut node = row(number);
    let extra = json!({
        "repository": { "id": REPOSITORY_ID },
        "mergeStateStatus": "CLEAN",
        "reviewThreads": {
            "pageInfo": { "hasNextPage": false },
            "nodes": [{ "isResolved": false }, { "isResolved": true }, { "isResolved": true }],
        },
        "comments": {
            "totalCount": 1,
            "nodes": [comment("6814185533", "octocat", "User")],
        },
        "commits": { "nodes": [{ "commit": {
            "oid": head(),
            "status": { "contexts": [{
                "context": "ci/deploy",
                "state": "SUCCESS",
                "description": "Deployed",
                "targetUrl": "https://ci.example/7",
                "createdAt": "2026-11-06T09:02:00Z",
            }] },
            "checkSuites": {
                "pageInfo": { "hasNextPage": false },
                "nodes": [
                    suite(
                        REPOSITORY_ID,
                        actions_run(500, 41),
                        json!([check_run(71, "build", "COMPLETED", Some("SUCCESS"))]),
                    ),
                    suite(
                        REPOSITORY_ID,
                        Value::Null,
                        json!([check_run(72, "review", "IN_PROGRESS", None)]),
                    ),
                ],
            },
        } }] },
    });
    for (field, value) in extra.as_object().unwrap() {
        node[field] = value.clone();
    }
    node
}

fn nodes_answer(nodes: Vec<Value>) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({ "data": { "nodes": nodes } }))
}

fn asked_ids(request: &Request) -> Vec<String> {
    let sent: Value = serde_json::from_slice(&request.body).unwrap();
    serde_json::from_value(sent["variables"]["ids"].clone()).unwrap()
}

fn asked_sizes(received: &[Request]) -> Vec<usize> {
    received
        .iter()
        .map(|request| asked_ids(request).len())
        .collect()
}

fn answering_each(request: &Request) -> ResponseTemplate {
    nodes_answer(
        asked_ids(request)
            .iter()
            .map(|id| evidence_node(number_of(id)))
            .collect(),
    )
}

async fn evidence_server(
    responder: impl Fn(&Request) -> ResponseTemplate + Send + Sync + 'static,
) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .respond_with(responder)
        .mount(&server)
        .await;
    server
}

fn repository_id() -> RepositoryId {
    RepositoryId::new(REPOSITORY_ID)
}

async fn evidence_from(
    server: &MockServer,
    ids: &[String],
) -> PullRequestResult<Vec<EvidenceSlot>> {
    let service = stand_in(server).await;
    service
        .fetch_pull_request_evidence(&project(&service), &token(), &repository_id(), ids)
        .await
}

async fn slot_for(node: Value) -> EvidenceSlot {
    let id = node["id"].as_str().unwrap().to_string();
    let server = evidence_server(move |_: &Request| nodes_answer(vec![node.clone()])).await;
    evidence_from(&server, &[id]).await.unwrap().pop().unwrap()
}

async fn evidence_for(node: Value) -> PullRequestEvidence {
    let slot = slot_for(node).await;
    slot.clone()
        .into_evidence()
        .unwrap_or_else(|| panic!("no evidence read: {slot:?}"))
}

fn kinds(slots: &[EvidenceSlot]) -> Vec<Option<&'static str>> {
    slots.iter().map(EvidenceSlot::reason).collect()
}

async fn evidence_failure(response: ResponseTemplate) -> PullRequestError {
    let server = evidence_server(move |_: &Request| response.clone()).await;
    evidence_from(&server, &identifiers([7])).await.unwrap_err()
}

fn open_answer(rows: Vec<Value>, next: bool, cursor: Option<&str>) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "data": { "repository": {
            "id": REPOSITORY_ID,
            "pullRequests": {
                "pageInfo": { "hasNextPage": next, "endCursor": cursor },
                "nodes": rows,
            },
        } },
    }))
}

async fn open_server(response: ResponseTemplate) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .respond_with(response)
        .mount(&server)
        .await;
    server
}

async fn open_from(
    server: &MockServer,
    after: Option<&str>,
) -> PullRequestResult<OpenPullRequestPage> {
    let service = stand_in(server).await;
    service
        .list_open_pull_requests(
            &project(&service),
            &token(),
            NonZeroU8::new(100).unwrap(),
            after,
        )
        .await
}

#[tokio::test]
async fn a_page_of_open_pull_requests_is_asked_for_by_its_cursor_and_size() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .and(header("authorization", "Bearer token"))
        .and(body_json(json!({
            "query": OPEN,
            "variables": { "owner": "acme", "name": "project", "first": 2, "after": "Y3Vyc29yOjE=" },
        })))
        .respond_with(open_answer(vec![row(9), row(7)], true, Some("Y3Vyc29yOjM=")))
        .expect(1)
        .mount(&server)
        .await;
    let service = stand_in(&server).await;

    let page = service
        .list_open_pull_requests(
            &project(&service),
            &token(),
            NonZeroU8::new(2).unwrap(),
            Some("Y3Vyc29yOjE="),
        )
        .await
        .unwrap();

    assert_eq!(page.repository_id, repository_id());
    assert_eq!(page.next.as_deref(), Some("Y3Vyc29yOjM="));
    let numbers: Vec<u64> = page
        .items
        .iter()
        .map(|listed| listed.reference.number().get())
        .collect();
    assert_eq!(numbers, vec![9, 7], "GitHub's order is kept");
    let first = &page.items[0];
    assert_eq!(first.reference.repository(), &project(&service));
    assert_eq!(first.node_id, "PR_9");
    assert_eq!(first.title, "change 9");
    assert_eq!(first.url, "https://github.com/acme/project/pull/9");
    assert_eq!(first.state, PullRequestState::Open);
    assert!(!first.draft);
    assert_eq!(first.head.as_str(), "task/9");
    assert_eq!(first.base.as_str(), "main");
    assert_eq!(
        first.head_sha.as_ref().map(CommitSha::as_str),
        Some(head().as_str())
    );
    assert_eq!(first.updated_at.as_deref(), Some(UPDATED));
    assert_eq!(first.author_login.as_deref(), Some("octocat"));
}

#[tokio::test]
async fn a_page_asks_for_no_more_than_a_hundred_rows_and_from_the_start_without_a_cursor() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .and(body_json(json!({
            "query": OPEN,
            "variables": { "owner": "acme", "name": "project", "first": 100, "after": null },
        })))
        .respond_with(open_answer(vec![row(7)], false, Some("Y3Vyc29yOjE=")))
        .expect(1)
        .mount(&server)
        .await;
    let service = stand_in(&server).await;

    let page = service
        .list_open_pull_requests(&project(&service), &token(), NonZeroU8::MAX, None)
        .await
        .unwrap();

    assert_eq!(page.next, None, "GitHub said no page follows");
    assert_eq!(page.items.len(), 1);
}

#[tokio::test]
async fn open_pull_requests_are_read_under_api_on_an_enterprise_origin() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/graphql"))
        .respond_with(open_answer(vec![row(7)], false, None))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/graphql"))
        .and(body_json(
            json!({ "query": EVIDENCE, "variables": { "ids": ["PR_7"] } }),
        ))
        .respond_with(nodes_answer(vec![evidence_node(7)]))
        .expect(1)
        .with_priority(1)
        .mount(&server)
        .await;
    let service = enterprise(&server).await;

    let page = service
        .list_open_pull_requests(&project(&service), &token(), NonZeroU8::MAX, None)
        .await
        .unwrap();
    let evidence = service
        .fetch_pull_request_evidence(
            &project(&service),
            &token(),
            &repository_id(),
            &identifiers([7]),
        )
        .await
        .unwrap();

    assert_eq!(page.items.len(), 1);
    assert!(evidence[0].evidence().is_some());
}

#[tokio::test]
async fn a_page_that_says_more_follow_without_a_new_cursor_is_an_error() {
    for (cursor, after) in [(None, None), (Some("Y3Vyc29yOjE="), Some("Y3Vyc29yOjE="))] {
        let server = open_server(open_answer(vec![row(7)], true, cursor)).await;

        let failure = open_from(&server, after).await.unwrap_err();

        assert!(
            matches!(failure, PullRequestError::GitHubApi(ref text) if text == NO_CURSOR),
            "{failure:?}"
        );
    }
}

#[tokio::test]
async fn a_repository_github_cannot_find_is_one_not_found() {
    for response in [
        ResponseTemplate::new(200).set_body_json(json!({ "data": { "repository": null } })),
        ResponseTemplate::new(200).set_body_json(json!({
            "data": { "repository": null },
            "errors": [{
                "type": "NOT_FOUND",
                "path": ["repository"],
                "message": "Could not resolve to a Repository with the name 'acme/project'.",
            }],
        })),
        ResponseTemplate::new(200).set_body_json(json!({
            "data": null,
            "errors": [{ "type": "NOT_FOUND", "path": ["repository"], "message": "gone" }],
        })),
    ] {
        let server = open_server(response).await;

        let failure = open_from(&server, None).await.unwrap_err();

        assert!(
            matches!(failure, PullRequestError::RepositoryNotFound),
            "{failure:?}"
        );
    }
}

#[tokio::test]
async fn a_schema_without_a_field_asked_for_is_a_github_error() {
    let server = open_server(ResponseTemplate::new(200).set_body_json(json!({
        "errors": [{
            "extensions": { "code": "undefinedField" },
            "message": "Field 'mergeStateStatus' doesn't exist on type 'PullRequest'",
        }],
    })))
    .await;

    let failure = open_from(&server, None).await.unwrap_err();

    assert!(
        matches!(failure, PullRequestError::GitHubApi(ref text) if text.contains("mergeStateStatus")),
        "{failure:?}"
    );
}

#[tokio::test]
async fn a_row_with_a_branch_this_crate_does_not_accept_fails_the_page() {
    let mut broken = row(8);
    broken["headRefName"] = json!("bad..branch");
    let server = open_server(open_answer(vec![row(7), broken], false, None)).await;

    let failure = open_from(&server, None).await.unwrap_err();

    assert!(matches!(failure, PullRequestError::Parse(_)), "{failure:?}");
}

#[tokio::test]
async fn an_apps_login_is_listed_as_rest_spells_it_and_a_deleted_one_as_ghost() {
    let mut app = row(8);
    app["author"] = json!({ "__typename": "Bot", "login": "renovate" });
    let mut ghost = row(9);
    ghost["author"] = Value::Null;
    let server = open_server(open_answer(vec![row(7), app, ghost], false, None)).await;

    let page = open_from(&server, None).await.unwrap();

    let authors: Vec<Option<&str>> = page
        .items
        .iter()
        .map(|listed| listed.author_login.as_deref())
        .collect();
    assert_eq!(
        authors,
        vec![Some("octocat"), Some("renovate[bot]"), Some("ghost")]
    );
}

fn firsts(received: &[Request]) -> Vec<u64> {
    received
        .iter()
        .map(|request| {
            let sent: Value = serde_json::from_slice(&request.body).unwrap();
            sent["variables"]["first"].as_u64().unwrap()
        })
        .collect()
}

#[tokio::test]
async fn a_page_too_slow_or_too_large_is_asked_for_again_at_half_as_many_rows() {
    for failing in [
        ResponseTemplate::new(504),
        ResponseTemplate::new(502),
        ResponseTemplate::new(200).set_body_json(json!({
            "data": null,
            "errors": [{ "message": GRAPHQL_TIMEOUT }],
        })),
        ResponseTemplate::new(200).set_body_json(json!({
            "data": null,
            "errors": [{ "type": "TIMEOUT", "message": "query took too long" }],
        })),
        ResponseTemplate::new(200).set_body_bytes(vec![b' '; MAXIMUM_ANSWER_BYTES + 1]),
    ] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .respond_with(move |request: &Request| {
                let sent: Value = serde_json::from_slice(&request.body).unwrap();
                match sent["variables"]["first"].as_u64() {
                    Some(100) => failing.clone(),
                    _ => open_answer(vec![row(7)], true, Some("Y3Vyc29yOjUw")),
                }
            })
            .mount(&server)
            .await;

        let page = open_from(&server, None).await.unwrap();

        assert_eq!(
            firsts(&server.received_requests().await.unwrap()),
            vec![100, 50]
        );
        assert_eq!(page.next.as_deref(), Some("Y3Vyc29yOjUw"));
    }
}

#[tokio::test]
async fn a_page_that_keeps_timing_out_is_halved_twice_and_then_fails() {
    let server = open_server(ResponseTemplate::new(504)).await;

    let failure = open_from(&server, None).await.unwrap_err();

    assert!(
        matches!(failure, PullRequestError::Refused { status: 504, .. }),
        "{failure:?}"
    );
    assert_eq!(
        firsts(&server.received_requests().await.unwrap()),
        vec![100, 50, 25]
    );
}

#[tokio::test]
async fn evidence_reads_everything_that_says_whether_a_pull_request_is_ready() {
    let read = evidence_for(evidence_node(7)).await;

    assert_eq!(read.pull_request.reference.number().get(), 7);
    assert_eq!(read.pull_request.node_id, "PR_7");
    assert_eq!(read.pull_request.author_login.as_deref(), Some("octocat"));
    assert_eq!(read.mergeable_state, MergeableState::Clean);
    assert_eq!(read.threads, ReviewThreadCount::new(1, 2, true));
    assert_eq!(
        read.comments,
        Collected::new(
            vec![IssueComment::new(
                6_814_185_533,
                "octocat",
                "looks good",
                "https://github.com/acme/project/pull/7#issuecomment-6814185533",
                "2026-11-06T09:10:00Z",
            )],
            true,
        )
    );
    let sha = CommitSha::parse(&head()).unwrap();
    let checks = read.head.unwrap();
    assert_eq!(checks.sha, sha);
    assert_eq!(
        checks.workflow_runs,
        Collected::new(
            vec![
                WorkflowRun::new(
                    NonZeroU64::new(500).unwrap(),
                    NonZeroU64::new(41).unwrap(),
                    ".github/workflows/ci.yml",
                    "pull_request",
                    sha.clone(),
                    "2026-11-06T09:00:00Z",
                    "2026-11-06T09:06:00Z",
                    "https://github.com/acme/project/actions/runs/500",
                    "acme/project",
                )
                .with_name("CI")
                .with_status("completed")
                .with_conclusion("success")
                .with_head_branch("task/7")
            ],
            true,
        )
    );
    assert_eq!(
        checks.check_runs,
        Collected::new(
            vec![
                CheckRun::new("completed")
                    .with_id(71)
                    .with_name("build")
                    .with_head_sha(sha.clone())
                    .with_conclusion("success")
                    .with_url("https://github.com/acme/project/actions/runs/500/job/71")
                    .with_details_url("https://github.com/acme/project/actions/runs/500/job/71")
                    .with_started_at("2026-11-06T09:00:00Z")
                    .with_completed_at("2026-11-06T09:05:00Z"),
                CheckRun::new("in_progress")
                    .with_id(72)
                    .with_name("review")
                    .with_head_sha(sha)
                    .with_url("https://github.com/acme/project/runs/72")
                    .with_details_url("https://github.com/acme/project/actions/runs/500/job/72")
                    .with_started_at("2026-11-06T09:00:00Z")
                    .with_completed_at("2026-11-06T09:05:00Z"),
            ],
            true,
        )
    );
    assert_eq!(
        checks.statuses,
        Collected::new(
            vec![
                CommitStatus::new("ci/deploy", "success")
                    .with_description("Deployed")
                    .with_target_url("https://ci.example/7")
                    .with_created_at("2026-11-06T09:02:00Z")
            ],
            true,
        )
    );
}

#[tokio::test]
async fn evidence_asks_for_its_pull_requests_by_id_as_a_variable() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/graphql"))
        .and(header("authorization", "Bearer token"))
        .and(body_json(json!({
            "query": EVIDENCE,
            "variables": { "ids": ["PR_7", "PR_9"] },
        })))
        .respond_with(nodes_answer(vec![evidence_node(7), evidence_node(9)]))
        .expect(1)
        .mount(&server)
        .await;

    let evidence = evidence_from(&server, &identifiers([7, 9])).await.unwrap();

    let numbers: Vec<Option<u64>> = evidence
        .iter()
        .map(|slot| {
            slot.evidence()
                .map(|read| read.pull_request.reference.number().get())
        })
        .collect();
    assert_eq!(numbers, vec![Some(7), Some(9)]);
}

#[tokio::test]
async fn an_apps_comment_is_named_as_rest_names_it_and_a_deleted_authors_as_ghost() {
    let mut node = evidence_node(7);
    node["comments"] = json!({
        "totalCount": 2,
        "nodes": [
            comment("11", "review-app", "Bot"),
            {
                "fullDatabaseId": 12,
                "body": null,
                "url": "https://github.com/acme/project/pull/7#issuecomment-12",
                "createdAt": "2026-11-06T09:11:00Z",
                "author": null,
            },
        ],
    });
    node["author"] = json!({ "__typename": "Bot", "login": "renovate" });

    let read = evidence_for(node).await;

    assert_eq!(
        read.pull_request.author_login.as_deref(),
        Some("renovate[bot]")
    );
    let comments: Vec<(u64, &str, &str)> = read
        .comments
        .items
        .iter()
        .map(|comment| (comment.id, comment.author.as_str(), comment.body.as_str()))
        .collect();
    assert_eq!(
        comments,
        vec![(11, "review-app[bot]", "looks good"), (12, "ghost", "")]
    );
    assert!(read.comments.complete);
}

#[tokio::test]
async fn every_merge_state_reads_as_rest_spells_it_whether_or_not_it_is_a_draft() {
    for (status, draft, expected) in [
        (json!("BEHIND"), false, MergeableState::Behind),
        (json!("BLOCKED"), false, MergeableState::Blocked),
        (json!("BLOCKED"), true, MergeableState::Blocked),
        (json!("CLEAN"), false, MergeableState::Clean),
        (json!("DIRTY"), false, MergeableState::Dirty),
        (json!("DIRTY"), true, MergeableState::Dirty),
        (json!("DRAFT"), true, MergeableState::Draft),
        (json!("HAS_HOOKS"), false, MergeableState::HasHooks),
        (json!("UNSTABLE"), false, MergeableState::Unstable),
        (json!("UNSTABLE"), true, MergeableState::Unstable),
        (json!("UNKNOWN"), false, MergeableState::Unknown),
        (json!("SOMETHING_NEW"), false, MergeableState::Unknown),
        (Value::Null, false, MergeableState::Unknown),
    ] {
        let mut node = evidence_node(7);
        node["mergeStateStatus"] = status.clone();
        node["isDraft"] = json!(draft);

        let read = evidence_for(node).await;

        assert_eq!(read.mergeable_state, expected, "{status} draft {draft}");
        assert_eq!(read.pull_request.draft, draft, "{status}");
    }
}

#[tokio::test]
async fn a_list_cut_short_says_so() {
    let mut threads = evidence_node(7);
    threads["reviewThreads"]["pageInfo"]["hasNextPage"] = json!(true);
    let read = evidence_for(threads).await;
    assert!(!read.threads.complete, "threads past the first hundred");
    assert!(read.comments.complete);
    assert!(read.head.unwrap().check_runs.complete);

    let mut comments = evidence_node(7);
    comments["comments"]["totalCount"] = json!(101);
    let read = evidence_for(comments).await;
    assert!(!read.comments.complete, "a hundred and one comments");
    assert!(read.threads.complete);

    let mut suites = evidence_node(7);
    suites["commits"]["nodes"][0]["commit"]["checkSuites"]["pageInfo"]["hasNextPage"] = json!(true);
    let read = evidence_for(suites).await;
    let checks = read.head.unwrap();
    assert!(
        !checks.workflow_runs.complete,
        "suites past the first fifty"
    );
    assert!(!checks.check_runs.complete, "suites past the first fifty");
    assert!(checks.statuses.complete);

    let mut runs = evidence_node(7);
    runs["commits"]["nodes"][0]["commit"]["checkSuites"]["nodes"][1]["checkRuns"]["pageInfo"]["hasNextPage"] =
        json!(true);
    let read = evidence_for(runs).await;
    let checks = read.head.unwrap();
    assert!(
        !checks.check_runs.complete,
        "a suite's runs past its first hundred"
    );
    assert!(checks.workflow_runs.complete);

    let mut unpaged = evidence_node(7);
    unpaged["commits"]["nodes"][0]["commit"]["checkSuites"]["nodes"][0]["checkRuns"] = Value::Null;
    let read = evidence_for(unpaged).await;
    assert!(
        !read.head.unwrap().check_runs.complete,
        "a suite whose runs GitHub left out"
    );
}

#[tokio::test]
async fn suites_from_a_fork_are_left_out_and_suites_without_a_run_list_no_run() {
    let mut node = evidence_node(7);
    node["commits"]["nodes"][0]["commit"]["checkSuites"]["nodes"]
        .as_array_mut()
        .unwrap()
        .push(suite(
            FORK_ID,
            actions_run(900, 90),
            json!([check_run(91, "fork build", "COMPLETED", Some("FAILURE"))]),
        ));

    let checks = evidence_for(node).await.head.unwrap();

    let runs: Vec<u64> = checks
        .workflow_runs
        .items
        .iter()
        .map(|run| run.id.get())
        .collect();
    assert_eq!(
        runs,
        vec![500],
        "the suite without a run lists none, the fork's is left out"
    );
    let check_runs: Vec<Option<u64>> = checks.check_runs.items.iter().map(|run| run.id).collect();
    assert_eq!(check_runs, vec![Some(71), Some(72)]);
}

#[tokio::test]
async fn a_rerun_lists_its_run_once_at_its_newest_suite_and_runs_newest_first() {
    let mut first_attempt = suite(REPOSITORY_ID, actions_run(600, 41), json!([]));
    first_attempt["conclusion"] = json!("FAILURE");
    first_attempt["createdAt"] = json!("2026-11-06T08:00:00Z");
    let mut second_attempt = suite(REPOSITORY_ID, actions_run(600, 41), json!([]));
    second_attempt["status"] = json!("IN_PROGRESS");
    second_attempt["conclusion"] = Value::Null;
    second_attempt["createdAt"] = json!("2026-11-06T10:00:00Z");
    let older = suite(REPOSITORY_ID, actions_run(550, 42), json!([]));
    let mut node = evidence_node(7);
    node["commits"]["nodes"][0]["commit"]["checkSuites"]["nodes"] =
        json!([first_attempt, older, second_attempt]);

    let checks = evidence_for(node).await.head.unwrap();

    let runs: Vec<(u64, Option<&str>, Option<&str>)> = checks
        .workflow_runs
        .items
        .iter()
        .map(|run| {
            (
                run.id.get(),
                run.status.as_deref(),
                run.conclusion.as_deref(),
            )
        })
        .collect();
    assert_eq!(
        runs,
        vec![
            (600, Some("in_progress"), None),
            (550, Some("completed"), Some("success")),
        ]
    );
}

#[tokio::test]
async fn a_status_still_expected_is_left_out_and_no_status_reads_as_none() {
    let mut expected = evidence_node(7);
    expected["commits"]["nodes"][0]["commit"]["status"]["contexts"]
        .as_array_mut()
        .unwrap()
        .push(json!({ "context": "required/gate", "state": "EXPECTED" }));
    let checks = evidence_for(expected).await.head.unwrap();
    let contexts: Vec<(&str, &str)> = checks
        .statuses
        .items
        .iter()
        .map(|status| (status.context.as_str(), status.state.as_str()))
        .collect();
    assert_eq!(contexts, vec![("ci/deploy", "success")]);

    let mut silent = evidence_node(7);
    silent["commits"]["nodes"][0]["commit"]["status"] = Value::Null;
    let checks = evidence_for(silent).await.head.unwrap();
    assert_eq!(checks.statuses, Collected::new(Vec::new(), true));
}

#[tokio::test]
async fn an_error_reported_at_an_entry_fails_it_even_with_data() {
    let server = evidence_server(|_: &Request| {
        ResponseTemplate::new(200).set_body_json(json!({
            "data": { "nodes": [evidence_node(7), evidence_node(8)] },
            "errors": [{
                "type": "FORBIDDEN",
                "path": ["nodes", 0, "commits"],
                "message": "Resource not accessible by integration",
            }],
        }))
    })
    .await;

    let evidence = evidence_from(&server, &identifiers([7, 8])).await.unwrap();

    assert_eq!(kinds(&evidence), vec![Some("reported"), None]);
}

#[tokio::test]
async fn a_renamed_repository_is_known_by_its_id_not_its_name() {
    let mut renamed = evidence_node(7);
    renamed["repository"] = json!({ "id": REPOSITORY_ID, "nameWithOwner": "acme/renamed" });
    renamed["url"] = json!("https://github.com/acme/renamed/pull/7");

    let read = evidence_for(renamed).await;

    assert_eq!(
        read.pull_request.url,
        "https://github.com/acme/renamed/pull/7"
    );
    assert!(read.head.is_some());
}

#[tokio::test]
async fn a_refusal_of_the_whole_read_fails_every_pull_request_in_it() {
    for (response, expected) in [
        (
            ResponseTemplate::new(200).set_body_json(json!({
                "data": null,
                "errors": [{ "type": "RATE_LIMITED", "message": "API rate limit exceeded" }],
            })),
            "RateLimited",
        ),
        (
            ResponseTemplate::new(200).set_body_json(json!({
                "data": { "nodes": [evidence_node(7)] },
                "errors": [{ "type": "RATE_LIMITED", "path": ["nodes", 0], "message": "slow down" }],
            })),
            "RateLimited",
        ),
        (
            ResponseTemplate::new(403).set_body_json(json!({ "message": SECONDARY_LIMIT })),
            "RateLimited",
        ),
        (
            ResponseTemplate::new(200).set_body_json(json!({
                "data": { "nodes": [evidence_node(7)] },
                "errors": [{ "type": "FORBIDDEN", "message": "Resource not accessible" }],
            })),
            "Forbidden",
        ),
        (
            ResponseTemplate::new(200).set_body_json(json!({
                "data": null,
                "errors": [{ "type": "NOT_FOUND", "message": "gone" }],
            })),
            "NotFound",
        ),
        (ResponseTemplate::new(401), "AuthenticationFailed"),
        (
            ResponseTemplate::new(200).set_body_json(json!({ "data": null })),
            "GitHubApi(\"GitHub's GraphQL API answered with no data\")",
        ),
        (
            ResponseTemplate::new(200).set_body_json(json!({ "data": { "nodes": [] } })),
            "GitHubApi(\"GitHub answered in a form this crate cannot read\")",
        ),
    ] {
        let failure = evidence_failure(response).await;

        assert_eq!(format!("{failure:?}"), expected);
    }
}

fn failing_over(
    size: usize,
    failure: ResponseTemplate,
) -> impl Fn(&Request) -> ResponseTemplate + Send + Sync + 'static {
    move |request: &Request| match asked_ids(request).len() > size {
        true => failure.clone(),
        false => answering_each(request),
    }
}

#[tokio::test]
async fn more_than_a_batch_is_refused_unsent_and_none_asks_for_nothing() {
    let server = evidence_server(answering_each).await;

    let none = evidence_from(&server, &[]).await.unwrap();
    let failure = evidence_from(&server, &identifiers(1..=11))
        .await
        .unwrap_err();
    let full = evidence_from(&server, &identifiers(1..=10)).await.unwrap();

    assert!(none.is_empty());
    assert!(
        matches!(
            failure,
            PullRequestError::BatchTooLarge {
                asked: 11,
                maximum: 10
            }
        ),
        "{failure:?}"
    );
    assert_eq!(full.len(), 10);
    let received = server.received_requests().await.unwrap();
    assert_eq!(
        asked_sizes(&received),
        vec![10],
        "only the batch that fits is sent"
    );
}

#[tokio::test]
async fn a_pull_request_github_could_not_read_fails_only_its_own_slot() {
    let mut unreadable = evidence_node(8);
    unreadable["reviewThreads"] = json!("RAW-SENTINEL");
    let mut foreign = evidence_node(9);
    foreign["repository"]["id"] = json!(FORK_ID);
    let mut another = evidence_node(10);
    another["id"] = json!("PR_99");
    let mut unparsed = evidence_node(11);
    unparsed["headRefOid"] = json!("not-a-commit");
    let server = evidence_server(move |_: &Request| {
        ResponseTemplate::new(200).set_body_json(json!({
            "data": { "nodes": [
                evidence_node(6),
                Value::Null,
                unreadable.clone(),
                foreign.clone(),
                another.clone(),
                unparsed.clone(),
                evidence_node(12),
                {},
                Value::Null,
            ] },
            "errors": [{
                "type": "NOT_FOUND",
                "path": ["nodes", 1],
                "message": "Could not resolve to a node with the global id of 'PR_7'",
            }],
        }))
    })
    .await;

    let evidence = evidence_from(&server, &identifiers(6..=14)).await.unwrap();

    assert_eq!(
        kinds(&evidence),
        vec![
            None,
            Some("reported"),
            Some("unreadable"),
            Some("foreign"),
            Some("foreign"),
            Some("unreadable"),
            None,
            Some("unreadable"),
            Some("missing"),
        ]
    );
}

#[tokio::test]
async fn a_comment_whose_id_cannot_be_read_leaves_its_pull_request_unreadable() {
    let mut node = evidence_node(7);
    node["comments"]["nodes"] = json!([comment("not-a-number", "octocat", "User")]);

    assert_eq!(slot_for(node).await, EvidenceSlot::Unreadable);
}

#[tokio::test]
async fn a_newest_commit_that_is_not_the_head_reads_no_checks() {
    let mut moved = evidence_node(7);
    moved["commits"]["nodes"][0]["commit"]["oid"] = json!("b".repeat(40));
    let mut empty = evidence_node(7);
    empty["commits"]["nodes"] = json!([]);
    let mut headless = evidence_node(7);
    headless["headRefOid"] = Value::Null;

    for node in [moved, empty, headless] {
        let read = evidence_for(node).await;

        assert_eq!(read.head, None);
        assert_eq!(read.threads, ReviewThreadCount::new(1, 2, true));
    }
}

#[tokio::test]
async fn copilot_is_named_as_rest_names_it() {
    let mut node = evidence_node(7);
    node["author"] = json!({ "__typename": "Bot", "login": "copilot-swe-agent" });
    node["comments"]["nodes"] = json!([comment("11", "copilot-swe-agent", "Bot")]);

    let read = evidence_for(node).await;

    assert_eq!(read.pull_request.author_login.as_deref(), Some("Copilot"));
    assert_eq!(read.comments.items[0].author, "Copilot");
}

#[tokio::test]
async fn a_merged_or_closed_pull_request_reads_as_closed() {
    for (state, expected) in [
        ("OPEN", PullRequestState::Open),
        ("CLOSED", PullRequestState::Closed),
        ("MERGED", PullRequestState::Closed),
        ("ARCHIVED", PullRequestState::Unknown),
    ] {
        let mut node = evidence_node(7);
        node["state"] = json!(state);

        assert_eq!(
            evidence_for(node).await.pull_request.state,
            expected,
            "{state}"
        );
    }
}

#[tokio::test]
async fn threads_whose_page_says_nothing_of_what_follows_are_incomplete() {
    let mut node = evidence_node(7);
    node["reviewThreads"]
        .as_object_mut()
        .unwrap()
        .remove("pageInfo")
        .unwrap();

    let read = evidence_for(node).await;

    assert_eq!(read.threads, ReviewThreadCount::new(1, 2, false));
}

#[tokio::test]
async fn an_actions_run_graphql_says_little_of_reads_with_what_rest_would_say() {
    let mut run = actions_run(500, 41);
    run["file"] = Value::Null;
    let mut job = check_run(71, "analyze", "COMPLETED", Some("SUCCESS"));
    job["detailsUrl"] = Value::Null;
    let mut bare = suite(REPOSITORY_ID, run, json!([job]));
    bare["branch"] = Value::Null;
    let mut node = evidence_node(7);
    node["commits"]["nodes"][0]["commit"]["checkSuites"]["nodes"] = json!([bare]);

    let checks = evidence_for(node.clone()).await.head.unwrap();

    let run = &checks.workflow_runs.items[0];
    assert_eq!(run.path, "", "a dynamic workflow names no file");
    assert_eq!(
        run.head_branch.as_deref(),
        Some("task/7"),
        "a fork's suite names no branch, so the pull request's head stands in"
    );
    assert_eq!(run.attempt, None);
    assert_eq!(run.started_at, None);
    let job = &checks.check_runs.items[0];
    assert_eq!(job.url, None, "an Actions job links where its details are");
    assert_eq!(job.details_url, None);
    assert!(checks.workflow_runs.complete);

    node["commits"]["nodes"][0]["commit"]["checkSuites"]["nodes"][0]["workflowRun"]["event"] =
        json!("dynamic");
    let checks = evidence_for(node).await.head.unwrap();
    assert_eq!(
        checks.workflow_runs.items[0].head_branch, None,
        "a dynamic run names no branch for the head to stand in for"
    );
}

#[tokio::test]
async fn an_actions_suite_graphql_names_no_run_for_leaves_the_runs_incomplete() {
    let mut runless = suite(REPOSITORY_ID, Value::Null, json!([]));
    runless["app"] = json!({ "slug": "github-actions" });
    let mut node = evidence_node(7);
    node["commits"]["nodes"][0]["commit"]["checkSuites"]["nodes"]
        .as_array_mut()
        .unwrap()
        .push(runless);

    let checks = evidence_for(node).await.head.unwrap();

    assert!(!checks.workflow_runs.complete);
    assert!(checks.check_runs.complete);

    let mut fork_runless = suite(FORK_ID, Value::Null, json!([]));
    fork_runless["app"] = json!({ "slug": "github-actions" });
    let mut forked = evidence_node(7);
    forked["commits"]["nodes"][0]["commit"]["checkSuites"]["nodes"]
        .as_array_mut()
        .unwrap()
        .push(fork_runless);
    let checks = evidence_for(forked).await.head.unwrap();
    assert!(
        checks.workflow_runs.complete,
        "a fork's suite is left out, run or not"
    );
}

#[tokio::test]
async fn a_page_refused_with_data_beside_it_is_that_refusal() {
    for (response, expected) in [
        (
            ResponseTemplate::new(200).set_body_json(json!({
                "data": { "repository": null },
                "errors": [{ "type": "RATE_LIMITED", "message": "API rate limit exceeded" }],
            })),
            "RateLimited",
        ),
        (
            ResponseTemplate::new(200).set_body_json(json!({
                "data": null,
                "errors": [{ "type": "RATE_LIMITED", "message": "rate limit hit before the query timed out" }],
            })),
            "RateLimited",
        ),
        (
            ResponseTemplate::new(403).set_body_json(json!({ "message": SECONDARY_LIMIT })),
            "RateLimited",
        ),
        (
            ResponseTemplate::new(200).set_body_json(json!({
                "data": { "repository": { "id": REPOSITORY_ID, "pullRequests": {
                    "pageInfo": { "hasNextPage": false, "endCursor": null },
                    "nodes": [],
                } } },
                "errors": [{ "type": "FORBIDDEN", "message": "Resource not accessible" }],
            })),
            "Forbidden",
        ),
    ] {
        let server = open_server(response).await;

        let failure = open_from(&server, None).await.unwrap_err();

        assert_eq!(format!("{failure:?}"), expected);
        assert_eq!(
            server.received_requests().await.unwrap().len(),
            1,
            "a refusal is never halved"
        );
    }
}

#[tokio::test]
async fn a_read_too_slow_or_too_large_is_asked_for_again_in_halves() {
    for failure in [
        ResponseTemplate::new(504),
        ResponseTemplate::new(502),
        ResponseTemplate::new(200).set_body_json(json!({
            "data": null,
            "errors": [{ "message": GRAPHQL_TIMEOUT }],
        })),
        ResponseTemplate::new(200).set_body_json(json!({
            "data": null,
            "errors": [{ "type": "TIMEOUT", "message": "query took too long" }],
        })),
        ResponseTemplate::new(200).set_body_bytes(vec![b' '; MAXIMUM_ANSWER_BYTES + 1]),
    ] {
        let server = evidence_server(failing_over(5, failure)).await;

        let evidence = evidence_from(&server, &identifiers(1..=10)).await.unwrap();

        let numbers: Vec<Option<u64>> = evidence
            .iter()
            .map(|slot| {
                slot.evidence()
                    .map(|read| read.pull_request.reference.number().get())
            })
            .collect();
        assert_eq!(
            numbers,
            (1..=10).map(Some).collect::<Vec<Option<u64>>>(),
            "each in the order asked"
        );
        let received = server.received_requests().await.unwrap();
        assert_eq!(asked_sizes(&received), vec![10, 5, 5]);
    }
}

#[tokio::test]
async fn a_read_is_halved_at_most_twice() {
    let server = evidence_server(failing_over(3, ResponseTemplate::new(504))).await;

    let evidence = evidence_from(&server, &identifiers(1..=10)).await.unwrap();

    assert!(evidence.iter().all(|slot| slot.evidence().is_some()));
    let received = server.received_requests().await.unwrap();
    assert_eq!(asked_sizes(&received), vec![10, 5, 2, 3, 5, 2, 3]);
}

#[tokio::test]
async fn three_pull_requests_split_into_one_and_two_and_two_into_one_each() {
    let server = evidence_server(failing_over(1, ResponseTemplate::new(504))).await;

    let evidence = evidence_from(&server, &identifiers(1..=3)).await.unwrap();

    assert!(evidence.iter().all(|slot| slot.evidence().is_some()));
    let received = server.received_requests().await.unwrap();
    assert_eq!(asked_sizes(&received), vec![3, 1, 2, 1, 1]);
}

#[tokio::test]
async fn a_pull_request_github_cannot_answer_for_leaves_only_its_part_unanswered() {
    let server =
        evidence_server(
            |request: &Request| match asked_ids(request).contains(&identifier(4)) {
                true => ResponseTemplate::new(504),
                false => answering_each(request),
            },
        )
        .await;

    let evidence = evidence_from(&server, &identifiers(1..=10)).await.unwrap();

    let unanswered = Some("unanswered");
    assert_eq!(
        kinds(&evidence),
        vec![
            None, None, unanswered, unanswered, unanswered, None, None, None, None, None
        ],
        "the half read keeps its evidence after the other half fails"
    );
    let received = server.received_requests().await.unwrap();
    assert_eq!(asked_sizes(&received), vec![10, 5, 2, 3, 5]);
}

#[tokio::test]
async fn a_read_that_keeps_timing_out_is_unanswered_after_at_most_seven_requests() {
    let server = evidence_server(|_: &Request| ResponseTemplate::new(504)).await;

    let evidence = evidence_from(&server, &identifiers(1..=10)).await.unwrap();

    assert!(
        evidence
            .iter()
            .all(|slot| *slot == EvidenceSlot::Unanswered)
    );
    let received = server.received_requests().await.unwrap();
    assert_eq!(asked_sizes(&received), vec![10, 5, 2, 3, 5, 2, 3]);

    let timing_out = evidence_server(|_: &Request| {
        ResponseTemplate::new(200).set_body_json(json!({
            "data": null,
            "errors": [{ "message": GRAPHQL_TIMEOUT }],
        }))
    })
    .await;
    let evidence = evidence_from(&timing_out, &identifiers(1..=4))
        .await
        .unwrap();
    assert!(
        evidence
            .iter()
            .all(|slot| *slot == EvidenceSlot::Unanswered)
    );
    let received = timing_out.received_requests().await.unwrap();
    assert_eq!(asked_sizes(&received), vec![4, 2, 1, 1, 2, 1, 1]);
}

#[tokio::test]
async fn a_refusal_of_a_later_half_fails_the_call_though_an_earlier_half_was_read() {
    let server = evidence_server(|request: &Request| {
        let asked = asked_ids(request);
        if asked.len() == 10 {
            return ResponseTemplate::new(504);
        }
        match asked.contains(&identifier(6)) {
            true => ResponseTemplate::new(200).set_body_json(json!({
                "errors": [{ "type": "RATE_LIMITED", "message": "API rate limit exceeded" }],
            })),
            false => answering_each(request),
        }
    })
    .await;

    let failure = evidence_from(&server, &identifiers(1..=10))
        .await
        .unwrap_err();

    assert!(
        matches!(failure, PullRequestError::RateLimited),
        "{failure:?}"
    );
    let received = server.received_requests().await.unwrap();
    assert_eq!(asked_sizes(&received), vec![10, 5, 5]);
}

#[tokio::test]
async fn a_failure_that_is_not_a_timeout_is_never_halved() {
    for response in [
        ResponseTemplate::new(500),
        ResponseTemplate::new(200).set_body_json(json!({
            "data": null,
            "errors": [{ "message": "Something else went wrong" }],
        })),
    ] {
        let server = evidence_server(move |_: &Request| response.clone()).await;

        evidence_from(&server, &identifiers(1..=10))
            .await
            .unwrap_err();

        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }
}
