use super::*;
use crate::pull_request::Collected;
use crate::pull_request::ReviewThreadCount;
use crate::pull_request::ReviewThreadRecord;
use crate::pull_request::ThreadComment;
use crate::pull_request::graphql_page_info::GraphQlPageInfo;
use crate::pull_request::graphql_pull_request::GraphQlPullRequest;
use crate::pull_request::graphql_query::GraphQlQuery;
use crate::pull_request::graphql_repository::GraphQlRepository;
use crate::pull_request::graphql_review_thread::GraphQlReviewThread;
use crate::pull_request::graphql_thread_comment::GraphQlThreadComment;
use crate::pull_request::graphql_thread_state::GraphQlThreadState;
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

/// Reads the page of a pull request's review threads that follows the cursor
/// `after`, each thread with only whether it is resolved.
const COUNTS: &str = "query($owner:String!,$name:String!,$number:Int!,$after:String){\
repository(owner:$owner,name:$name){pullRequest(number:$number){\
reviewThreads(first:100,after:$after){pageInfo{hasNextPage endCursor}\
nodes{id isResolved}}}}}";

/// A review thread as one page of a GraphQL answer holds it, known by the
/// node id that tells a thread met twice from two threads.
trait Identified {
    fn id(&self) -> &str;
}

impl Identified for GraphQlReviewThread {
    fn id(&self) -> &str {
        &self.id
    }
}

impl Identified for GraphQlThreadState {
    fn id(&self) -> &str {
        &self.id
    }
}

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
        self.graphql::<IgnoredAny, _>(token, RESOLVE, &json!({ "id": thread }))
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
    /// partial or empty list. [`Self::collect_review_threads`] reads the same
    /// and says whether that is every thread.
    pub async fn fetch_review_threads(
        &self,
        reference: &PullRequestReference,
        token: &SecretValue,
    ) -> PullRequestResult<Vec<ReviewThreadRecord>> {
        Ok(self.collect_review_threads(reference, token).await?.items)
    }

    /// Every review thread [`Self::fetch_review_threads`] reads, in the order
    /// GitHub listed them, and whether that is every thread on the pull
    /// request.
    ///
    /// [`Collected::complete`] is `true` only when the last page read was one
    /// GitHub said no page follows. It is `false` when the walk stopped at the
    /// page limit while GitHub said another page follows, when such a page
    /// named no cursor or handed back the cursor it was just given, or when a
    /// page said nothing of what follows it, since a thread not reached may be
    /// the one still unresolved. It speaks for the threads alone: a thread
    /// carries at most its first twenty comments either way. A failure is the
    /// one [`Self::fetch_review_threads`] reports, never a partial list.
    pub async fn collect_review_threads(
        &self,
        reference: &PullRequestReference,
        token: &SecretValue,
    ) -> PullRequestResult<Collected<ReviewThreadRecord>> {
        let collected: Collected<GraphQlReviewThread> =
            self.review_thread_nodes(reference, token, THREADS).await?;
        Ok(Collected::new(
            collected.items.into_iter().map(record).collect(),
            collected.complete,
        ))
    }

    /// How many of a pull request's review threads are open, and how many
    /// are resolved, without reading a comment in any of them.
    ///
    /// Reads the threads [`Self::fetch_review_threads`] would, page by page
    /// and with the same limit, counting a thread met twice once, and fails
    /// the same way it does. Each thread costs GitHub a node id and a flag,
    /// so a pull request with long review conversations costs no more to
    /// count than one without.
    pub async fn fetch_review_thread_counts(
        &self,
        reference: &PullRequestReference,
        token: &SecretValue,
    ) -> PullRequestResult<ReviewThreadCount> {
        let collected: Collected<GraphQlThreadState> =
            self.review_thread_nodes(reference, token, COUNTS).await?;
        Ok(collected
            .items
            .iter()
            .fold(ReviewThreadCount::default(), |count, thread| {
                count.with(thread.is_resolved)
            }))
    }

    /// Every review thread `query` reads a page of, following GitHub's
    /// cursor, with a thread met twice kept once, and whether that is every
    /// thread on the pull request.
    async fn review_thread_nodes<T: DeserializeOwned + Identified>(
        &self,
        reference: &PullRequestReference,
        token: &SecretValue,
        query: &'static str,
    ) -> PullRequestResult<Collected<T>> {
        let repository = reference.repository();
        let mut threads: Vec<T> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        let mut after: Option<String> = None;
        let mut complete = false;

        for _ in 0..MAXIMUM_PAGES {
            let answer: GraphQlQuery<GraphQlRepository<GraphQlPullRequest<T>>> = self
                .graphql(
                    token,
                    query,
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
                .and_then(|found| found.pull_request)
                .ok_or(PullRequestError::NotFound)?
                .review_threads;

            threads.extend(
                page.nodes
                    .into_iter()
                    .filter(|thread| seen.insert(thread.id().to_string())),
            );
            complete = last(page.page_info.as_ref());
            match next_cursor(page.page_info, after.as_deref()) {
                Some(cursor) => after = Some(cursor),
                None => break,
            }
        }

        Ok(Collected::new(threads, complete))
    }
}

/// Whether GitHub said no page follows the one that ended at `page`.
fn last(page: Option<&GraphQlPageInfo>) -> bool {
    page.is_some_and(|page| !page.has_next_page)
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
        sent(THREADS, after)
    }

    fn counted(after: Option<&str>) -> Value {
        sent(COUNTS, after)
    }

    fn sent(query: &str, after: Option<&str>) -> Value {
        json!({
            "query": query,
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

    fn threads(first: usize, count: usize) -> Value {
        (first..first + count)
            .map(|number| thread(&format!("PRRT_{number}")))
            .collect()
    }

    #[tokio::test]
    async fn full_pages_that_run_out_are_every_thread() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .and(body_json(asked(None)))
            .respond_with(page(threads(0, PAGE_SIZE), true, Some("Y3Vyc29yOjE=")))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .and(body_json(asked(Some("Y3Vyc29yOjE="))))
            .respond_with(page(
                threads(PAGE_SIZE, PAGE_SIZE),
                true,
                Some("Y3Vyc29yOjI="),
            ))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .and(body_json(asked(Some("Y3Vyc29yOjI="))))
            .respond_with(page(
                threads(2 * PAGE_SIZE, PAGE_SIZE),
                false,
                Some("Y3Vyc29yOjM="),
            ))
            .expect(1)
            .mount(&server)
            .await;
        let service = stand_in(&server).await;

        let collected = service
            .collect_review_threads(&seven(&service), &token())
            .await
            .unwrap();

        assert!(collected.complete);
        assert_eq!(collected.items.len(), 3 * PAGE_SIZE);
        assert_eq!(collected.items[0].id, "PRRT_0");
        assert_eq!(collected.items[3 * PAGE_SIZE - 1].id, "PRRT_299");
    }

    #[tokio::test]
    async fn a_walk_the_page_limit_cuts_short_is_not_every_thread() {
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

        let collected = service
            .collect_review_threads(&seven(&service), &token())
            .await
            .unwrap();

        assert!(!collected.complete);
        assert_eq!(collected.items.len(), MAXIMUM_PAGES);
    }

    #[tokio::test]
    async fn a_last_page_read_while_another_follows_is_not_every_thread() {
        for (described, page_info) in [
            ("names no cursor", json!({ "hasNextPage": true })),
            (
                "hands back its cursor",
                json!({ "hasNextPage": true, "endCursor": "stuck" }),
            ),
            ("says nothing of what follows", Value::Null),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/graphql"))
                .and(body_json(asked(None)))
                .respond_with(page(json!([thread("PRRT_1")]), true, Some("stuck")))
                .mount(&server)
                .await;
            Mock::given(method("POST"))
                .and(path("/graphql"))
                .and(body_json(asked(Some("stuck"))))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                    "data": { "repository": { "pullRequest": { "reviewThreads": {
                        "pageInfo": page_info,
                        "nodes": [thread("PRRT_2")],
                    } } } },
                })))
                .expect(1)
                .mount(&server)
                .await;
            let service = stand_in(&server).await;

            let collected = service
                .collect_review_threads(&seven(&service), &token())
                .await
                .unwrap();

            assert!(!collected.complete, "a last page that {described}");
            assert_eq!(identifiers(&collected.items), ["PRRT_1", "PRRT_2"]);
        }
    }

    #[tokio::test]
    async fn a_thread_holding_as_many_comments_as_are_read_leaves_the_threads_complete() {
        let mut crowded = thread("PRRT_1");
        let comment = crowded["comments"]["nodes"][0].clone();
        crowded["comments"]["nodes"] = Value::Array(vec![comment; 20]);
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .respond_with(page(json!([crowded]), false, None))
            .expect(1)
            .mount(&server)
            .await;
        let service = stand_in(&server).await;

        let collected = service
            .collect_review_threads(&seven(&service), &token())
            .await
            .unwrap();

        assert!(collected.complete);
        assert_eq!(collected.items[0].comments.len(), 20);
    }

    #[tokio::test]
    async fn resolved_threads_that_could_not_be_paginated_are_not_every_thread() {
        let mut resolved = thread("PRRT_1");
        resolved["isResolved"] = json!(true);
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": { "repository": { "pullRequest": { "reviewThreads": {
                    "pageInfo": { "hasNextPage": true },
                    "nodes": [resolved],
                } } } },
            })))
            .expect(1)
            .mount(&server)
            .await;
        let service = stand_in(&server).await;

        let collected = service
            .collect_review_threads(&seven(&service), &token())
            .await
            .unwrap();

        assert!(!collected.complete);
        assert!(collected.items.iter().all(|thread| thread.resolved));
        assert_eq!(identifiers(&collected.items), ["PRRT_1"]);
    }

    fn state(id: &str, resolved: bool) -> Value {
        json!({ "id": id, "isResolved": resolved })
    }

    #[test]
    fn counting_threads_asks_for_no_comments() {
        assert!(!COUNTS.contains("comments"), "{COUNTS}");
        assert!(COUNTS.contains("nodes{id isResolved}"), "{COUNTS}");
    }

    #[tokio::test]
    async fn review_threads_are_counted_open_and_resolved() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .and(header("authorization", "Bearer token"))
            .and(body_json(counted(None)))
            .respond_with(page(
                json!([
                    state("PRRT_1", false),
                    state("PRRT_2", true),
                    state("PRRT_3", false),
                    state("PRRT_4", true),
                    state("PRRT_5", true),
                ]),
                false,
                None,
            ))
            .expect(1)
            .mount(&server)
            .await;
        let service = stand_in(&server).await;

        let count = service
            .fetch_review_thread_counts(&seven(&service), &token())
            .await
            .unwrap();

        assert_eq!(count, ReviewThreadCount::new(2, 3));
    }

    #[tokio::test]
    async fn a_pull_request_without_threads_counts_none() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .respond_with(page(json!([]), false, None))
            .expect(1)
            .mount(&server)
            .await;
        let service = stand_in(&server).await;

        let count = service
            .fetch_review_thread_counts(&seven(&service), &token())
            .await
            .unwrap();

        assert_eq!(count, ReviewThreadCount::default());
    }

    #[tokio::test]
    async fn review_thread_counts_follow_the_cursor_to_the_last_page() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .and(body_json(counted(None)))
            .respond_with(page(
                json!([state("PRRT_1", false)]),
                true,
                Some("Y3Vyc29yOjE="),
            ))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .and(body_json(counted(Some("Y3Vyc29yOjE="))))
            .respond_with(page(
                json!([state("PRRT_2", true)]),
                false,
                Some("Y3Vyc29yOjI="),
            ))
            .expect(1)
            .mount(&server)
            .await;
        let service = stand_in(&server).await;

        let count = service
            .fetch_review_thread_counts(&seven(&service), &token())
            .await
            .unwrap();

        assert_eq!(count, ReviewThreadCount::new(1, 1));
    }

    #[tokio::test]
    async fn a_thread_counted_on_a_repeated_page_counts_once() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .respond_with(page(
                json!([state("PRRT_1", false), state("PRRT_2", true)]),
                true,
                Some("stuck"),
            ))
            .expect(2)
            .mount(&server)
            .await;
        let service = stand_in(&server).await;

        let count = service
            .fetch_review_thread_counts(&seven(&service), &token())
            .await
            .unwrap();

        assert_eq!(count, ReviewThreadCount::new(1, 1));
        let received = server.received_requests().await.unwrap();
        let sent: Value = serde_json::from_slice(&received[1].body).unwrap();
        assert_eq!(sent, counted(Some("stuck")));
    }

    #[tokio::test]
    async fn a_thread_on_two_overlapping_pages_counts_once() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .and(body_json(counted(None)))
            .respond_with(page(
                json!([state("PRRT_1", false), state("PRRT_2", true)]),
                true,
                Some("Y3Vyc29yOjE="),
            ))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .and(body_json(counted(Some("Y3Vyc29yOjE="))))
            .respond_with(page(
                json!([state("PRRT_2", true), state("PRRT_3", false)]),
                false,
                None,
            ))
            .expect(1)
            .mount(&server)
            .await;
        let service = stand_in(&server).await;

        let count = service
            .fetch_review_thread_counts(&seven(&service), &token())
            .await
            .unwrap();

        assert_eq!(count, ReviewThreadCount::new(2, 1));
    }

    #[tokio::test]
    async fn a_cursor_that_never_runs_out_is_counted_only_so_far() {
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
                    json!([state(&format!("PRRT_{next}"), false)]),
                    true,
                    Some(&next.to_string()),
                )
            })
            .expect(u64::try_from(MAXIMUM_PAGES).unwrap())
            .mount(&server)
            .await;
        let service = stand_in(&server).await;

        let count = service
            .fetch_review_thread_counts(&seven(&service), &token())
            .await
            .unwrap();

        assert_eq!(
            count,
            ReviewThreadCount::new(u32::try_from(MAXIMUM_PAGES).unwrap(), 0)
        );
    }

    #[tokio::test]
    async fn counting_a_pull_request_graphql_cannot_find_is_not_found() {
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
                .fetch_review_thread_counts(&seven(&service), &token())
                .await
                .unwrap_err();

            assert!(
                matches!(failure, PullRequestError::NotFound),
                "{body}: {failure:?}"
            );
        }
    }

    #[tokio::test]
    async fn a_counted_thread_that_does_not_say_whether_it_is_resolved_is_unreadable() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .respond_with(page(json!([{ "id": "PRRT_1" }]), false, None))
            .mount(&server)
            .await;
        let service = stand_in(&server).await;

        let failure = service
            .fetch_review_thread_counts(&seven(&service), &token())
            .await
            .unwrap_err();

        assert_eq!(format!("{failure:?}"), format!("GitHubApi({UNREADABLE:?})"));
    }
}
