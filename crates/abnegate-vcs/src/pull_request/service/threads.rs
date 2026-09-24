use super::*;
use crate::pull_request::ReviewThreadRecord;
use crate::pull_request::ThreadComment;
use crate::pull_request::graphql_page_info::GraphQlPageInfo;
use crate::pull_request::graphql_pull_request::GraphQlPullRequest;
use crate::pull_request::graphql_query::GraphQlQuery;
use crate::pull_request::graphql_repository::GraphQlRepository;
use crate::pull_request::graphql_review_thread::GraphQlReviewThread;
use crate::pull_request::graphql_thread_comment::GraphQlThreadComment;
use serde::de::IgnoredAny;
use serde_json::json;
use std::collections::HashSet;

/// Marks the review thread its `id` names resolved.
const RESOLVE: &str =
    "mutation($id:ID!){resolveReviewThread(input:{threadId:$id}){thread{isResolved}}}";

/// Reads the page of a pull request's review threads that follows the cursor
/// `after`, each thread with its first twenty comments.
const THREADS: &str = "query($owner:String!,$name:String!,$number:Int!,$after:String){\
repository(owner:$owner,name:$name){pullRequest(number:$number){\
reviewThreads(first:100,after:$after){pageInfo{hasNextPage endCursor}\
nodes{id isResolved isOutdated path line comments(first:20){\
nodes{databaseId body url createdAt author{login}}}}}}}}";

impl PullRequestService {
    /// Mark a review thread resolved.
    ///
    /// `thread` is the node id GitHub's GraphQL API names the thread by; it
    /// travels as a variable, never inside the mutation.
    pub async fn resolve_review_thread(
        &self,
        thread: &str,
        token: &SecretValue,
    ) -> PullRequestResult<()> {
        self.graphql::<IgnoredAny, _>(token, RESOLVE, &serde_json::json!({ "id": thread }))
            .await?;
        Ok(())
    }

    /// Every review thread on a pull request's diff, with the comments in it.
    ///
    /// Follows GitHub's cursor a page at a time until GitHub says no page
    /// follows, names no cursor, or hands back the cursor it was just given,
    /// and stops at the page limit every paged read keeps to. Each thread
    /// carries at most its first twenty comments, and a thread met twice is
    /// kept once. A repository or pull request GitHub cannot find, or will not
    /// show the token, is [`PullRequestError::NotFound`]. A page longer than
    /// the 16 MiB this crate reads is [`PullRequestError::GitHubApi`], never a
    /// partial or empty list.
    pub async fn fetch_review_threads(
        &self,
        reference: &PullRequestReference,
        token: &SecretValue,
    ) -> PullRequestResult<Vec<ReviewThreadRecord>> {
        let repository = reference.repository();
        let mut threads: Vec<ReviewThreadRecord> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        let mut after: Option<String> = None;

        for _ in 0..MAXIMUM_PAGES {
            let answer: GraphQlQuery<GraphQlRepository<GraphQlPullRequest>> = self
                .graphql(
                    token,
                    THREADS,
                    &json!({
                        "owner": repository.owner(),
                        "name": repository.name(),
                        "number": reference.number().get(),
                        "after": after,
                    }),
                )
                .await?;
            let page = answer
                .repository
                .and_then(|repository| repository.pull_request)
                .ok_or(PullRequestError::NotFound)?
                .review_threads;

            threads.extend(
                page.nodes
                    .into_iter()
                    .filter(|thread| seen.insert(thread.id.clone()))
                    .map(record),
            );
            match next_cursor(page.page_info, after.as_deref()) {
                Some(cursor) => after = Some(cursor),
                None => break,
            }
        }

        Ok(threads)
    }
}

/// The cursor to read the next page after: the one a page ended at, when
/// GitHub says another page follows and that cursor moved on from `after`.
fn next_cursor(page: Option<GraphQlPageInfo>, after: Option<&str>) -> Option<String> {
    page.filter(|page| page.has_next_page)
        .and_then(|page| page.end_cursor)
        .filter(|cursor| after != Some(cursor.as_str()))
}

/// A review thread as GitHub's GraphQL API answered for it, with its comments.
fn record(thread: GraphQlReviewThread) -> ReviewThreadRecord {
    ReviewThreadRecord {
        id: thread.id,
        resolved: thread.is_resolved,
        outdated: thread.is_outdated,
        path: thread.path,
        line: thread.line,
        comments: thread.comments.nodes.into_iter().map(comment).collect(),
    }
}

/// A comment in a review thread, with an empty author where GitHub names none.
fn comment(written: GraphQlThreadComment) -> ThreadComment {
    ThreadComment {
        database_id: written.database_id,
        author: written
            .author
            .and_then(|author| author.login)
            .unwrap_or_default(),
        body: written.body,
        url: written.url,
        created_at: written.created_at,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pull_request::service::fixtures::seven;
    use crate::pull_request::service::fixtures::stand_in;
    use crate::pull_request::service::fixtures::token;
    use serde_json::Value;
    use serde_json::json;
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::Request;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::body_json;
    use wiremock::matchers::header;
    use wiremock::matchers::method;
    use wiremock::matchers::path;

    #[tokio::test]
    async fn resolving_a_thread_sends_its_id_as_a_variable() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .and(header("authorization", "Bearer token"))
            .and(body_json(json!({
                "query": RESOLVE,
                "variables": { "id": "PRRT_1" },
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": { "resolveReviewThread": { "thread": { "isResolved": true } } },
            })))
            .expect(1)
            .mount(&server)
            .await;
        let service = stand_in(&server).await;

        service
            .resolve_review_thread("PRRT_1", &token())
            .await
            .unwrap();
    }

    fn asked(after: Option<&str>) -> Value {
        json!({
            "query": THREADS,
            "variables": {
                "owner": "acme",
                "name": "project",
                "number": 7,
                "after": after,
            },
        })
    }

    fn page(threads: Value, next: bool, cursor: Option<&str>) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(json!({
            "data": { "repository": { "pullRequest": { "reviewThreads": {
                "pageInfo": { "hasNextPage": next, "endCursor": cursor },
                "nodes": threads,
            } } } },
        }))
    }

    fn thread(id: &str) -> Value {
        json!({
            "id": id,
            "isResolved": false,
            "isOutdated": false,
            "path": "src/cart.ts",
            "line": 12,
            "comments": { "nodes": [{
                "databaseId": 99,
                "body": "handle the empty cart",
                "url": "https://github.com/acme/project/pull/7#discussion_r99",
                "createdAt": "2026-09-20T10:00:00Z",
                "author": { "login": "review-bot" },
            }] },
        })
    }

    fn identifiers(threads: &[ReviewThreadRecord]) -> Vec<&str> {
        threads.iter().map(|thread| thread.id.as_str()).collect()
    }

    #[tokio::test]
    async fn review_threads_are_read_with_their_comments() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .and(header("authorization", "Bearer token"))
            .and(body_json(asked(None)))
            .respond_with(page(
                json!([
                    thread("PRRT_1"),
                    {
                        "id": "PRRT_2",
                        "isResolved": true,
                        "isOutdated": true,
                        "path": "src/checkout.ts",
                        "line": null,
                        "comments": { "nodes": [{
                            "databaseId": null,
                            "body": "fixed",
                            "url": "https://github.com/acme/project/pull/7#discussion_r100",
                            "createdAt": "2026-09-21T09:30:00Z",
                            "author": null,
                        }] },
                    },
                ]),
                false,
                None,
            ))
            .expect(1)
            .mount(&server)
            .await;
        let service = stand_in(&server).await;

        let threads = service
            .fetch_review_threads(&seven(&service), &token())
            .await
            .unwrap();

        assert_eq!(
            threads,
            vec![
                ReviewThreadRecord {
                    id: "PRRT_1".to_string(),
                    resolved: false,
                    outdated: false,
                    path: Some("src/cart.ts".to_string()),
                    line: Some(12),
                    comments: vec![ThreadComment {
                        database_id: Some(99),
                        author: "review-bot".to_string(),
                        body: "handle the empty cart".to_string(),
                        url: "https://github.com/acme/project/pull/7#discussion_r99".to_string(),
                        created_at: "2026-09-20T10:00:00Z".to_string(),
                    }],
                },
                ReviewThreadRecord {
                    id: "PRRT_2".to_string(),
                    resolved: true,
                    outdated: true,
                    path: Some("src/checkout.ts".to_string()),
                    line: None,
                    comments: vec![ThreadComment {
                        database_id: None,
                        author: String::new(),
                        body: "fixed".to_string(),
                        url: "https://github.com/acme/project/pull/7#discussion_r100".to_string(),
                        created_at: "2026-09-21T09:30:00Z".to_string(),
                    }],
                },
            ]
        );
    }

    #[tokio::test]
    async fn review_threads_follow_the_cursor_to_the_last_page() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .and(body_json(asked(None)))
            .respond_with(page(json!([thread("PRRT_1")]), true, Some("Y3Vyc29yOjE=")))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .and(body_json(asked(Some("Y3Vyc29yOjE="))))
            .respond_with(page(json!([thread("PRRT_2")]), false, Some("Y3Vyc29yOjI=")))
            .expect(1)
            .mount(&server)
            .await;
        let service = stand_in(&server).await;

        let threads = service
            .fetch_review_threads(&seven(&service), &token())
            .await
            .unwrap();

        assert_eq!(identifiers(&threads), ["PRRT_1", "PRRT_2"]);
    }

    #[tokio::test]
    async fn a_cursor_that_does_not_advance_ends_the_walk() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .respond_with(page(json!([thread("PRRT_1")]), true, Some("stuck")))
            .expect(2)
            .mount(&server)
            .await;
        let service = stand_in(&server).await;

        let threads = service
            .fetch_review_threads(&seven(&service), &token())
            .await
            .unwrap();

        assert_eq!(identifiers(&threads), ["PRRT_1"]);
        let received = server.received_requests().await.unwrap();
        let sent: Value = serde_json::from_slice(&received[1].body).unwrap();
        assert_eq!(sent, asked(Some("stuck")));
    }

    #[tokio::test]
    async fn a_page_that_names_no_cursor_ends_the_walk() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .respond_with(page(json!([thread("PRRT_1")]), true, None))
            .expect(1)
            .mount(&server)
            .await;
        let service = stand_in(&server).await;

        let threads = service
            .fetch_review_threads(&seven(&service), &token())
            .await
            .unwrap();

        assert_eq!(identifiers(&threads), ["PRRT_1"]);
    }

    #[tokio::test]
    async fn a_cursor_that_never_runs_out_is_followed_only_so_far() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .respond_with(|request: &Request| {
                let sent: Value = serde_json::from_slice(&request.body).unwrap();
                let read = sent["variables"]["after"]
                    .as_str()
                    .map_or(0, |cursor| cursor.parse::<usize>().unwrap());
                let next = read + 1;
                page(
                    json!([thread(&format!("PRRT_{next}"))]),
                    true,
                    Some(&next.to_string()),
                )
            })
            .expect(u64::try_from(MAXIMUM_PAGES).unwrap())
            .mount(&server)
            .await;
        let service = stand_in(&server).await;

        let threads = service
            .fetch_review_threads(&seven(&service), &token())
            .await
            .unwrap();

        assert_eq!(threads.len(), MAXIMUM_PAGES);
    }

    #[tokio::test]
    async fn a_pull_request_graphql_cannot_find_is_not_found() {
        for body in [
            json!({ "data": { "repository": null } }),
            json!({ "data": { "repository": { "pullRequest": null } } }),
            json!({
                "data": { "repository": { "pullRequest": null } },
                "errors": [{
                    "type": "NOT_FOUND",
                    "path": ["repository", "pullRequest"],
                    "message": "Could not resolve to a PullRequest with the number of 7.",
                }],
            }),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/graphql"))
                .respond_with(ResponseTemplate::new(200).set_body_json(&body))
                .expect(1)
                .mount(&server)
                .await;
            let service = stand_in(&server).await;

            let failure = service
                .fetch_review_threads(&seven(&service), &token())
                .await
                .unwrap_err();

            assert!(
                matches!(failure, PullRequestError::NotFound),
                "{body}: {failure:?}"
            );
        }
    }

    #[tokio::test]
    async fn a_thread_missing_what_the_query_asked_for_is_unreadable() {
        let mut incomplete = thread("PRRT_1");
        incomplete
            .as_object_mut()
            .unwrap()
            .remove("isResolved")
            .unwrap();
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .respond_with(page(json!([incomplete]), false, None))
            .mount(&server)
            .await;
        let service = stand_in(&server).await;

        let failure = service
            .fetch_review_threads(&seven(&service), &token())
            .await
            .unwrap_err();

        assert_eq!(format!("{failure:?}"), format!("GitHubApi({UNREADABLE:?})"));
    }
}
