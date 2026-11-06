use super::*;
use crate::pull_request::Collected;
use crate::pull_request::ReviewThreadCount;
use crate::pull_request::ReviewThreadRecord;
use crate::pull_request::ThreadComment;
use crate::pull_request::graphql_error::GraphQlError;
use crate::pull_request::graphql_identified::GraphQlIdentified;
use crate::pull_request::graphql_page_info::GraphQlPageInfo;
use crate::pull_request::graphql_pull_request::GraphQlPullRequest;
use crate::pull_request::graphql_query::GraphQlQuery;
use crate::pull_request::graphql_repository::GraphQlRepository;
use crate::pull_request::graphql_review_thread::GraphQlReviewThread;
use crate::pull_request::graphql_thread_comment::GraphQlThreadComment;
use crate::pull_request::graphql_thread_state::GraphQlThreadState;
use crate::pull_request::service::graphql::NOT_FOUND;
use crate::pull_request::service::graphql::RATE_LIMITED;
use crate::pull_request::service::graphql::rejection;
use crate::pull_request::service::graphql::reported;
use serde::de::IgnoredAny;
use serde_json::json;
use std::collections::HashSet;

/// Marks the review thread its `id` names resolved.
const RESOLVE: &str =
    "mutation($id:ID!){resolveReviewThread(input:{threadId:$id}){thread{isResolved}}}";

/// Reads the page of `first` of a pull request's review threads that follows
/// the cursor `after`, and the number of the pull request GitHub answered for,
/// each thread with the lines and side it was left on and its first hundred
/// comments.
const THREADS: &str = "query($owner:String!,$name:String!,$number:Int!,$first:Int!,$after:String){\
repository(owner:$owner,name:$name){pullRequest(number:$number){number \
reviewThreads(first:$first,after:$after){pageInfo{hasNextPage endCursor}\
nodes{id isResolved isOutdated path line startLine originalLine originalStartLine diffSide \
comments(first:100){pageInfo{hasNextPage}\
nodes{databaseId body url createdAt author{login}}}}}}}}";

/// Reads the page of `first` of a pull request's review threads that follows
/// the cursor `after`, and the number of the pull request GitHub answered for,
/// each thread with only whether it is resolved.
const COUNTS: &str = "query($owner:String!,$name:String!,$number:Int!,$first:Int!,$after:String){\
repository(owner:$owner,name:$name){pullRequest(number:$number){number \
reviewThreads(first:$first,after:$after){pageInfo{hasNextPage endCursor}\
nodes{id isResolved}}}}}";

/// Where in a review thread query GitHub reports the repository it could not
/// find.
const REPOSITORY_PATH: &[&str] = &["repository"];

/// Where in a review thread query GitHub reports the pull request it could
/// not find.
const PULL_REQUEST_PATH: &[&str] = &["repository", "pullRequest"];

/// Review threads a walk asks for before it stops: the reach of
/// [`MAXIMUM_PAGES`] pages of [`PAGE_SIZE`], kept whatever size halving leaves
/// a page at.
const MAXIMUM_THREADS: usize = PAGE_SIZE * MAXIMUM_PAGES;

/// Sizes halving asks a page of review threads for, from [`PAGE_SIZE`] down
/// to one: a hundred, fifty, twenty-five, twelve, six, three and one.
const PAGE_SIZES: usize = PAGE_SIZE.ilog2() as usize + 1;

/// Requests a walk sends before it stops, a page asked for again included:
/// as many as [`MAXIMUM_PAGES`] pages each halved through every one of
/// [`PAGE_SIZES`] would send, so a thread too heavy to read beside its
/// neighbours shortens the walk rather than turning it into a request a
/// thread.
const MAXIMUM_REQUESTS: usize = MAXIMUM_PAGES * PAGE_SIZES;

/// What a page of review threads GitHub answered for another pull request,
/// or for one it did not number, is reported as.
const ANOTHER_PULL_REQUEST: &str =
    "GitHub answered for a pull request other than the one asked for";

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

    /// Every review thread on a pull request's diff, with the lines and side
    /// it was left on and the first hundred comments in it.
    ///
    /// Follows GitHub's cursor a page at a time until GitHub says no page
    /// follows, names no cursor, or hands back the cursor it was just given,
    /// and stops once its pages have asked for a thousand threads in all, the
    /// reach of the ten pages of a hundred every paged read keeps to. A page
    /// asks for a hundred threads; one whose answer is larger than the 16 MiB
    /// this crate reads is asked for again from the same cursor at half as
    /// many, down to one, and each page read after it asks for twice as many
    /// as the one before, back up to a hundred, so only the pages near a heavy
    /// thread stay small. A page asked for again does not count towards the
    /// thousand; the last page asks only for what is left of it. However its
    /// pages were sized, the walk sends at most seventy requests, those asked
    /// for again included, as many as ten pages each halved from a hundred
    /// down to one would; one that reaches them stops where it is, keeping the
    /// threads already read, even when the page it was halving had yet to
    /// fit. Each thread carries at most its first hundred comments, and says
    /// in [`ReviewThreadRecord::comments_complete`] whether that is all of
    /// them; a thread met twice is kept once.
    ///
    /// A pull request with more than a thousand threads reads as its first
    /// thousand, or fewer when the walk spent its seventy requests first, and
    /// the list does not say that more were left;
    /// [`Self::collect_review_threads`] reads the same and says whether that
    /// is every thread.
    ///
    /// A repository GitHub cannot find, or will not show the token, is
    /// [`PullRequestError::RepositoryNotFound`], and a pull request the
    /// repository does not have is [`PullRequestError::PullRequestNotFound`];
    /// a missing thing GitHub names no part of is
    /// [`PullRequestError::NotFound`].
    /// [`PullRequestError::is_not_found`] is `true` for all three. Every page
    /// must name the pull request asked for: one that names another, or none,
    /// is [`PullRequestError::GitHubApi`]. A page of one thread still larger
    /// than the 16 MiB this crate reads is [`PullRequestError::GitHubApi`] too.
    /// Either way the read fails whole, never with a partial or empty list.
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
    /// GitHub said no page follows. It is `false` when the walk stopped at a
    /// thousand threads or at seventy requests while GitHub said another page
    /// follows, when such a page named no cursor or handed back the cursor it
    /// was just given, or when a page said nothing of what follows it, since a
    /// thread not reached may be the one still unresolved. It speaks for the
    /// threads alone: a thread carries at most its first hundred comments
    /// either way, and [`ReviewThreadRecord::comments_complete`] says whether
    /// those are all of them. Pages are asked for at a hundred threads, halved
    /// while an answer is larger than this crate reads and grown back after,
    /// as [`Self::fetch_review_threads`] sizes them. A failure is the one
    /// [`Self::fetch_review_threads`] reports, including
    /// [`PullRequestError::RepositoryNotFound`],
    /// [`PullRequestError::PullRequestNotFound`] and a page that names
    /// another pull request, never a partial list.
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
    /// Reads the threads [`Self::collect_review_threads`] would, page by page,
    /// stopping at the same thousand threads or seventy requests and halving a
    /// page whose answer is larger than this crate reads and growing the next
    /// back the same way, counting a thread met twice once, and fails the same
    /// way it does:
    /// [`PullRequestError::RepositoryNotFound`] and
    /// [`PullRequestError::PullRequestNotFound`] tell a hidden repository from
    /// a missing pull request, [`PullRequestError::is_not_found`] is `true`
    /// for either, and a page that names another pull request, or none, fails
    /// the count whole. [`ReviewThreadCount::complete`] is `true` exactly when
    /// [`Collected::complete`] would be: when the walk stops short, the
    /// threads it never reached are in neither count. Each thread costs GitHub
    /// a node id and a flag, so a pull request with long review conversations
    /// costs no more to count than one without.
    pub async fn fetch_review_thread_counts(
        &self,
        reference: &PullRequestReference,
        token: &SecretValue,
    ) -> PullRequestResult<ReviewThreadCount> {
        let collected: Collected<GraphQlThreadState> =
            self.review_thread_nodes(reference, token, COUNTS).await?;
        let empty = ReviewThreadCount::new(0, 0, collected.complete);
        Ok(collected
            .items
            .iter()
            .fold(empty, |count, thread| count.with(thread.is_resolved)))
    }

    /// Every review thread `query` reads a page of, following GitHub's
    /// cursor, with a thread met twice kept once, and whether that is every
    /// thread on the pull request.
    async fn review_thread_nodes<T: DeserializeOwned + GraphQlIdentified>(
        &self,
        reference: &PullRequestReference,
        token: &SecretValue,
        query: &'static str,
    ) -> PullRequestResult<Collected<T>> {
        let repository = reference.repository();
        let number = reference.number().get();
        let mut threads: Vec<T> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        let mut after: Option<String> = None;
        let mut first = PAGE_SIZE;
        let mut remaining = MAXIMUM_THREADS;
        let mut requests = 0;
        let mut complete = false;

        'walk: while remaining > 0 {
            let (answer, asked): (
                GraphQlQuery<GraphQlRepository<GraphQlPullRequest<T>>>,
                usize,
            ) = loop {
                if requests == MAXIMUM_REQUESTS {
                    break 'walk;
                }
                requests += 1;
                let asked = first.min(remaining);
                let read = self
                    .graphql_refusing(
                        token,
                        query,
                        &json!({
                            "owner": repository.owner(),
                            "name": repository.name(),
                            "number": number,
                            "first": asked,
                            "after": after,
                        }),
                        thread_rejection,
                    )
                    .await;
                match read {
                    Err(PullRequestError::GitHubApi(ref message))
                        if message == OVERSIZED && asked > 1 =>
                    {
                        first = asked / 2;
                    }
                    read => break (read?, asked),
                }
            };
            remaining -= asked;
            first = (asked * 2).min(PAGE_SIZE);
            let pull_request = answer
                .repository
                .ok_or(PullRequestError::RepositoryNotFound)?
                .pull_request
                .ok_or(PullRequestError::PullRequestNotFound)?;
            if pull_request.number != Some(number) {
                return Err(PullRequestError::GitHubApi(
                    ANOTHER_PULL_REQUEST.to_string(),
                ));
            }
            let page = pull_request.review_threads;

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

/// The refusal a set of GraphQL errors from a review thread query names: a
/// spent rate limit first, then a pull request and then a repository GitHub
/// says it could not find, and every other refusal as any GraphQL call reads
/// it.
fn thread_rejection(errors: &[GraphQlError]) -> PullRequestError {
    if reported(errors, RATE_LIMITED) {
        return PullRequestError::RateLimited;
    }
    if missing(errors, PULL_REQUEST_PATH) {
        return PullRequestError::PullRequestNotFound;
    }
    if missing(errors, REPOSITORY_PATH) {
        return PullRequestError::RepositoryNotFound;
    }
    rejection(errors)
}

/// Whether any of a set of GraphQL errors says nothing was found at exactly
/// `path` in the query.
fn missing(errors: &[GraphQlError], path: &[&str]) -> bool {
    errors.iter().any(|error| {
        error.kind.as_deref() == Some(NOT_FOUND)
            && error.path.as_deref().is_some_and(|segments| {
                segments
                    .iter()
                    .map(serde_json::Value::as_str)
                    .eq(path.iter().copied().map(Some))
            })
    })
}

/// Whether GitHub said no page follows the one that ended at `page`, rather
/// than that one does or nothing at all.
fn last(page: Option<&GraphQlPageInfo>) -> bool {
    page.and_then(|page| page.has_next_page) == Some(false)
}

/// The cursor to read the next page after: the one a page ended at, when
/// GitHub says another page follows and that cursor moved on from `after`.
fn next_cursor(page: Option<GraphQlPageInfo>, after: Option<&str>) -> Option<String> {
    page.filter(|page| page.has_next_page == Some(true))
        .and_then(|page| page.end_cursor)
        .filter(|cursor| after != Some(cursor.as_str()))
}

/// A review thread as GitHub's GraphQL API answered for it, with its comments
/// and whether they are all of them.
fn record(thread: GraphQlReviewThread) -> ReviewThreadRecord {
    ReviewThreadRecord {
        id: thread.id,
        resolved: thread.is_resolved,
        outdated: thread.is_outdated,
        path: thread.path,
        line: thread.line,
        start_line: thread.start_line,
        original_line: thread.original_line,
        original_start_line: thread.original_start_line,
        side: thread.diff_side.unwrap_or_default(),
        comments_complete: last(thread.comments.page_info.as_ref()),
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
    use crate::pull_request::DiffSide;
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
        sent(THREADS, PAGE_SIZE, after)
    }

    fn counted(after: Option<&str>) -> Value {
        sent(COUNTS, PAGE_SIZE, after)
    }

    fn sent(query: &str, first: usize, after: Option<&str>) -> Value {
        json!({
            "query": query,
            "variables": {
                "owner": "acme",
                "name": "project",
                "number": 7,
                "first": first,
                "after": after,
            },
        })
    }

    fn page(threads: Value, next: bool, cursor: Option<&str>) -> ResponseTemplate {
        numbered(json!(7), threads, next, cursor)
    }

    fn numbered(
        number: Value,
        threads: Value,
        next: bool,
        cursor: Option<&str>,
    ) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(json!({
            "data": { "repository": { "pullRequest": {
                "number": number,
                "reviewThreads": {
                    "pageInfo": { "hasNextPage": next, "endCursor": cursor },
                    "nodes": threads,
                },
            } } },
        }))
    }

    fn thread(id: &str) -> Value {
        json!({
            "id": id,
            "isResolved": false,
            "isOutdated": false,
            "path": "src/cart.ts",
            "line": 12,
            "startLine": null,
            "originalLine": 12,
            "originalStartLine": null,
            "diffSide": "RIGHT",
            "comments": {
                "pageInfo": { "hasNextPage": false },
                "nodes": [{
                    "databaseId": 99,
                    "body": "handle the empty cart",
                    "url": "https://github.com/acme/project/pull/7#discussion_r99",
                    "createdAt": "2026-09-20T10:00:00Z",
                    "author": { "login": "review-bot" },
                }],
            },
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
                        "startLine": null,
                        "originalLine": 30,
                        "originalStartLine": 27,
                        "diffSide": "LEFT",
                        "comments": {
                            "pageInfo": { "hasNextPage": false },
                            "nodes": [{
                                "databaseId": null,
                                "body": "fixed",
                                "url": "https://github.com/acme/project/pull/7#discussion_r100",
                                "createdAt": "2026-09-21T09:30:00Z",
                                "author": null,
                            }],
                        },
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
                    start_line: None,
                    original_line: Some(12),
                    original_start_line: None,
                    side: DiffSide::Right,
                    comments: vec![ThreadComment {
                        database_id: Some(99),
                        author: "review-bot".to_string(),
                        body: "handle the empty cart".to_string(),
                        url: "https://github.com/acme/project/pull/7#discussion_r99".to_string(),
                        created_at: "2026-09-20T10:00:00Z".to_string(),
                    }],
                    comments_complete: true,
                },
                ReviewThreadRecord {
                    id: "PRRT_2".to_string(),
                    resolved: true,
                    outdated: true,
                    path: Some("src/checkout.ts".to_string()),
                    line: None,
                    start_line: None,
                    original_line: Some(30),
                    original_start_line: Some(27),
                    side: DiffSide::Left,
                    comments: vec![ThreadComment {
                        database_id: None,
                        author: String::new(),
                        body: "fixed".to_string(),
                        url: "https://github.com/acme/project/pull/7#discussion_r100".to_string(),
                        created_at: "2026-09-21T09:30:00Z".to_string(),
                    }],
                    comments_complete: true,
                },
            ]
        );
    }

    #[test]
    fn reading_threads_asks_for_their_lines_side_and_first_hundred_comments() {
        assert!(
            THREADS.contains("comments(first:100){pageInfo{hasNextPage}"),
            "{THREADS}"
        );
        assert!(
            THREADS.contains("startLine originalLine originalStartLine diffSide"),
            "{THREADS}"
        );
        assert!(
            THREADS.contains("pullRequest(number:$number){number "),
            "{THREADS}"
        );
        assert!(
            COUNTS.contains("pullRequest(number:$number){number "),
            "{COUNTS}"
        );
    }

    async fn read_alone(thread: Value) -> Collected<ReviewThreadRecord> {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .respond_with(page(json!([thread]), false, None))
            .expect(1)
            .mount(&server)
            .await;
        let service = stand_in(&server).await;

        service
            .collect_review_threads(&seven(&service), &token())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn a_thread_on_a_range_reads_where_the_range_starts() {
        let mut ranged = thread("PRRT_1");
        ranged["startLine"] = json!(10);
        ranged["originalLine"] = json!(14);
        ranged["originalStartLine"] = json!(11);

        let collected = read_alone(ranged).await;

        let read = &collected.items[0];
        assert_eq!(read.line, Some(12));
        assert_eq!(read.start_line, Some(10));
        assert_eq!(read.original_line, Some(14));
        assert_eq!(read.original_start_line, Some(11));
    }

    #[tokio::test]
    async fn a_thread_on_one_line_or_naming_none_reads_no_range() {
        let mut absent = thread("PRRT_1");
        for field in ["startLine", "originalLine", "originalStartLine"] {
            absent.as_object_mut().unwrap().remove(field).unwrap();
        }
        for (described, answered) in [("null", thread("PRRT_1")), ("absent", absent)] {
            let collected = read_alone(answered).await;

            let read = &collected.items[0];
            assert_eq!(read.start_line, None, "a start line that is {described}");
            assert_eq!(
                read.original_start_line, None,
                "an original start line that is {described}"
            );
        }
    }

    #[tokio::test]
    async fn a_thread_reads_the_side_github_names() {
        for (answered, side) in [
            (json!("LEFT"), DiffSide::Left),
            (json!("RIGHT"), DiffSide::Right),
            (json!("CENTER"), DiffSide::Unknown),
            (Value::Null, DiffSide::Unknown),
        ] {
            let mut sided = thread("PRRT_1");
            sided["diffSide"] = answered.clone();

            let collected = read_alone(sided).await;

            assert_eq!(collected.items[0].side, side, "{answered}");
        }

        let mut unsided = thread("PRRT_1");
        unsided.as_object_mut().unwrap().remove("diffSide").unwrap();
        let collected = read_alone(unsided).await;
        assert_eq!(collected.items[0].side, DiffSide::Unknown);
    }

    #[tokio::test]
    async fn a_thread_with_comments_left_unread_is_incomplete_but_leaves_the_threads_complete() {
        for (described, page_info) in [
            ("says more follow", json!({ "hasNextPage": true })),
            ("says nothing of what follows", Value::Null),
        ] {
            let mut crowded = thread("PRRT_1");
            crowded["comments"]["pageInfo"] = page_info;

            let collected = read_alone(crowded).await;

            assert!(
                !collected.items[0].comments_complete,
                "a thread whose comments page {described}"
            );
            assert!(
                collected.complete,
                "the threads stay complete when a thread's comments page {described}"
            );
        }

        let mut unpaged = thread("PRRT_1");
        unpaged["comments"]
            .as_object_mut()
            .unwrap()
            .remove("pageInfo")
            .unwrap();
        let collected = read_alone(unpaged).await;
        assert!(
            !collected.items[0].comments_complete,
            "a thread whose comments page carries no page info"
        );
        assert!(collected.complete);
    }

    #[tokio::test]
    async fn a_page_for_another_pull_request_fails_the_read() {
        for (described, number) in [("another number", json!(8)), ("a null number", Value::Null)] {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/graphql"))
                .respond_with(numbered(number, json!([thread("PRRT_1")]), false, None))
                .expect(1)
                .mount(&server)
                .await;
            let service = stand_in(&server).await;

            let failure = service
                .collect_review_threads(&seven(&service), &token())
                .await
                .unwrap_err();

            assert_eq!(
                format!("{failure:?}"),
                format!("GitHubApi({ANOTHER_PULL_REQUEST:?})"),
                "a page that names {described}"
            );
        }
    }

    #[tokio::test]
    async fn a_page_without_a_number_fails_the_read() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": { "repository": { "pullRequest": { "reviewThreads": {
                    "pageInfo": { "hasNextPage": false, "endCursor": null },
                    "nodes": [thread("PRRT_1")],
                } } } },
            })))
            .expect(1)
            .mount(&server)
            .await;
        let service = stand_in(&server).await;

        let failure = service
            .collect_review_threads(&seven(&service), &token())
            .await
            .unwrap_err();

        assert_eq!(
            format!("{failure:?}"),
            format!("GitHubApi({ANOTHER_PULL_REQUEST:?})")
        );
    }

    #[tokio::test]
    async fn a_later_page_for_another_pull_request_fails_the_read_whole() {
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
            .respond_with(numbered(json!(70), json!([thread("PRRT_2")]), false, None))
            .expect(1)
            .mount(&server)
            .await;
        let service = stand_in(&server).await;

        let failure = service
            .fetch_review_threads(&seven(&service), &token())
            .await
            .unwrap_err();

        assert_eq!(
            format!("{failure:?}"),
            format!("GitHubApi({ANOTHER_PULL_REQUEST:?})"),
            "a later page for another pull request fails the read, never returning the first page"
        );
    }

    #[tokio::test]
    async fn counting_a_page_for_another_pull_request_fails_the_count() {
        for number in [json!(8), Value::Null] {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/graphql"))
                .respond_with(numbered(
                    number.clone(),
                    json!([state("PRRT_1", false)]),
                    false,
                    None,
                ))
                .expect(1)
                .mount(&server)
                .await;
            let service = stand_in(&server).await;

            let failure = service
                .fetch_review_thread_counts(&seven(&service), &token())
                .await
                .unwrap_err();

            assert_eq!(
                format!("{failure:?}"),
                format!("GitHubApi({ANOTHER_PULL_REQUEST:?})"),
                "{number}"
            );
        }
    }

    fn missing_answers() -> Vec<(Value, &'static str)> {
        vec![
            (
                json!({ "data": { "repository": null } }),
                "RepositoryNotFound",
            ),
            (
                json!({
                    "data": { "repository": null },
                    "errors": [{
                        "type": "NOT_FOUND",
                        "path": ["repository"],
                        "message": "Could not resolve to a Repository with the name 'acme/project'.",
                    }],
                }),
                "RepositoryNotFound",
            ),
            (
                json!({ "data": { "repository": { "pullRequest": null } } }),
                "PullRequestNotFound",
            ),
            (
                json!({
                    "data": { "repository": { "pullRequest": null } },
                    "errors": [{
                        "type": "NOT_FOUND",
                        "path": ["repository", "pullRequest"],
                        "message": "Could not resolve to a PullRequest with the number of 7.",
                    }],
                }),
                "PullRequestNotFound",
            ),
            (
                json!({
                    "data": null,
                    "errors": [{
                        "type": "NOT_FOUND",
                        "message": "Could not resolve to a node.",
                    }],
                }),
                "NotFound",
            ),
            (
                json!({
                    "data": null,
                    "errors": [{
                        "type": "NOT_FOUND",
                        "path": null,
                        "message": "Could not resolve to a node.",
                    }],
                }),
                "NotFound",
            ),
            (
                json!({
                    "data": { "repository": null },
                    "errors": [
                        {
                            "type": "NOT_FOUND",
                            "path": ["repository"],
                            "message": "Could not resolve to a Repository with the name 'acme/project'.",
                        },
                        {
                            "type": "NOT_FOUND",
                            "path": ["repository", "pullRequest"],
                            "message": "Could not resolve to a PullRequest with the number of 7.",
                        },
                    ],
                }),
                "PullRequestNotFound",
            ),
            (
                json!({
                    "data": { "repository": { "pullRequest": null } },
                    "errors": [
                        {
                            "type": "NOT_FOUND",
                            "path": ["repository", "pullRequest"],
                            "message": "Could not resolve to a PullRequest with the number of 7.",
                        },
                        { "type": "RATE_LIMITED", "message": "API rate limit exceeded" },
                    ],
                }),
                "RateLimited",
            ),
        ]
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
    async fn a_repository_or_pull_request_graphql_cannot_find_is_told_apart() {
        for (body, expected) in missing_answers() {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/graphql"))
                .respond_with(ResponseTemplate::new(200).set_body_json(&body))
                .expect(1)
                .mount(&server)
                .await;
            let service = stand_in(&server).await;

            let failure = service
                .collect_review_threads(&seven(&service), &token())
                .await
                .unwrap_err();

            assert_eq!(format!("{failure:?}"), expected, "{body}");
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
                    "data": { "repository": { "pullRequest": {
                        "number": 7,
                        "reviewThreads": {
                            "pageInfo": page_info,
                            "nodes": [thread("PRRT_2")],
                        },
                    } } },
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
    async fn a_thread_holding_a_hundred_comments_and_no_more_is_complete() {
        let mut crowded = thread("PRRT_1");
        let comment = crowded["comments"]["nodes"][0].clone();
        crowded["comments"]["nodes"] = Value::Array(vec![comment; 100]);

        let collected = read_alone(crowded).await;

        assert!(collected.complete);
        assert_eq!(collected.items[0].comments.len(), 100);
        assert!(collected.items[0].comments_complete);
    }

    #[tokio::test]
    async fn resolved_threads_that_could_not_be_paginated_are_not_every_thread() {
        let mut resolved = thread("PRRT_1");
        resolved["isResolved"] = json!(true);
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": { "repository": { "pullRequest": {
                    "number": 7,
                    "reviewThreads": {
                        "pageInfo": { "hasNextPage": true },
                        "nodes": [resolved],
                    },
                } } },
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

        assert_eq!(count, ReviewThreadCount::new(2, 3, true));
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

        assert_eq!(count, ReviewThreadCount::new(0, 0, true));
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

        assert_eq!(count, ReviewThreadCount::new(1, 1, true));
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

        assert_eq!(count, ReviewThreadCount::new(1, 1, false));
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

        assert_eq!(count, ReviewThreadCount::new(2, 1, true));
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
            ReviewThreadCount::new(u32::try_from(MAXIMUM_PAGES).unwrap(), 0, false)
        );
    }

    #[tokio::test]
    async fn counting_a_repository_or_pull_request_graphql_cannot_find_tells_them_apart() {
        for (body, expected) in missing_answers() {
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

            assert_eq!(format!("{failure:?}"), expected, "{body}");
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

    #[tokio::test]
    async fn a_null_comment_page_flag_reads_the_thread_with_its_comments_incomplete() {
        let mut unflagged = thread("PRRT_1");
        unflagged["comments"]["pageInfo"] = json!({ "hasNextPage": null });

        let collected = read_alone(unflagged).await;

        assert!(!collected.items[0].comments_complete);
        assert!(collected.complete);
    }

    #[tokio::test]
    async fn a_null_thread_page_flag_ends_the_walk_short_of_every_thread() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": { "repository": { "pullRequest": {
                    "number": 7,
                    "reviewThreads": {
                        "pageInfo": { "hasNextPage": null, "endCursor": "Y3Vyc29yOjE=" },
                        "nodes": [thread("PRRT_1")],
                    },
                } } },
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
        assert_eq!(identifiers(&collected.items), ["PRRT_1"]);
    }

    fn oversized() -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_bytes(vec![b' '; MAXIMUM_ANSWER_BYTES + 1])
    }

    fn variables(received: &[Request], name: &str) -> Vec<Value> {
        received
            .iter()
            .map(|request| {
                let sent: Value = serde_json::from_slice(&request.body).unwrap();
                sent["variables"][name].clone()
            })
            .collect()
    }

    fn firsts(received: &[Request]) -> Vec<Value> {
        variables(received, "first")
    }

    #[tokio::test]
    async fn a_page_larger_than_the_crate_reads_is_asked_for_again_at_half_the_size() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .and(body_json(asked(None)))
            .respond_with(oversized())
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .and(body_json(sent(THREADS, 50, None)))
            .respond_with(page(json!([thread("PRRT_1")]), true, Some("Y3Vyc29yOjE=")))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .and(body_json(asked(Some("Y3Vyc29yOjE="))))
            .respond_with(page(json!([thread("PRRT_2")]), false, None))
            .expect(1)
            .mount(&server)
            .await;
        let service = stand_in(&server).await;

        let collected = service
            .collect_review_threads(&seven(&service), &token())
            .await
            .unwrap();

        assert!(collected.complete);
        assert_eq!(identifiers(&collected.items), ["PRRT_1", "PRRT_2"]);
        assert_eq!(
            firsts(&server.received_requests().await.unwrap()),
            [json!(100), json!(50), json!(100)],
            "the page after a halved one asks for twice as many"
        );
    }

    #[tokio::test]
    async fn a_later_page_larger_than_the_crate_reads_is_asked_for_again_from_the_same_cursor() {
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
            .respond_with(oversized())
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .and(body_json(sent(THREADS, 50, Some("Y3Vyc29yOjE="))))
            .respond_with(page(json!([thread("PRRT_2")]), false, None))
            .expect(1)
            .mount(&server)
            .await;
        let service = stand_in(&server).await;

        let collected = service
            .collect_review_threads(&seven(&service), &token())
            .await
            .unwrap();

        assert!(collected.complete);
        assert_eq!(identifiers(&collected.items), ["PRRT_1", "PRRT_2"]);
        let received = server.received_requests().await.unwrap();
        assert_eq!(firsts(&received), [json!(100), json!(100), json!(50)]);
        assert_eq!(
            variables(&received, "after"),
            [Value::Null, json!("Y3Vyc29yOjE="), json!("Y3Vyc29yOjE=")],
            "the halved page starts from the cursor the oversized one was asked from"
        );
    }

    #[tokio::test]
    async fn a_page_of_one_thread_larger_than_the_crate_reads_fails_the_read() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .respond_with(oversized())
            .expect(u64::try_from(PAGE_SIZES).unwrap())
            .mount(&server)
            .await;
        let service = stand_in(&server).await;

        let failure = service
            .collect_review_threads(&seven(&service), &token())
            .await
            .unwrap_err();

        assert!(
            matches!(failure, PullRequestError::GitHubApi(ref text) if text == OVERSIZED),
            "{failure:?}"
        );
        assert_eq!(
            firsts(&server.received_requests().await.unwrap()),
            [100, 50, 25, 12, 6, 3, 1].map(|first| json!(first)),
        );
    }

    fn halving_above(
        largest: usize,
        total: usize,
        node: fn(&str) -> Value,
    ) -> impl Fn(&Request) -> ResponseTemplate + Send + Sync + 'static {
        heavy_before(total, largest, total, node)
    }

    fn heavy_before(
        heavy: usize,
        largest: usize,
        total: usize,
        node: fn(&str) -> Value,
    ) -> impl Fn(&Request) -> ResponseTemplate + Send + Sync + 'static {
        move |request: &Request| {
            let sent: Value = serde_json::from_slice(&request.body).unwrap();
            let first = usize::try_from(sent["variables"]["first"].as_u64().unwrap()).unwrap();
            let offset = sent["variables"]["after"]
                .as_str()
                .map_or(0, |cursor| cursor.parse::<usize>().unwrap());
            if offset < heavy && first > largest {
                return oversized();
            }
            let end = offset + first.min(total - offset);
            page(
                (offset..end)
                    .map(|number| node(&format!("PRRT_{number}")))
                    .collect(),
                end < total,
                Some(&end.to_string()),
            )
        }
    }

    #[tokio::test]
    async fn a_walk_halved_to_fifty_reads_past_ten_pages() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .respond_with(halving_above(50, 750, thread))
            .expect(30)
            .mount(&server)
            .await;
        let service = stand_in(&server).await;

        let collected = service
            .collect_review_threads(&seven(&service), &token())
            .await
            .unwrap();

        assert!(collected.complete);
        assert_eq!(collected.items.len(), 750);
        assert_eq!(collected.items[749].id, "PRRT_749");
        assert_eq!(
            firsts(&server.received_requests().await.unwrap()),
            [100, 50]
                .repeat(15)
                .into_iter()
                .map(|first| json!(first))
                .collect::<Vec<_>>(),
            "every page grows back to a hundred and is halved again to fifty"
        );
    }

    #[tokio::test]
    async fn a_walk_halved_to_fifty_stops_at_a_thousand_threads() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .respond_with(halving_above(50, 5000, thread))
            .expect(39)
            .mount(&server)
            .await;
        let service = stand_in(&server).await;

        let collected = service
            .collect_review_threads(&seven(&service), &token())
            .await
            .unwrap();

        assert!(!collected.complete);
        assert_eq!(collected.items.len(), MAXIMUM_THREADS);
        assert_eq!(collected.items[MAXIMUM_THREADS - 1].id, "PRRT_999");
    }

    #[tokio::test]
    async fn the_last_page_of_a_halved_walk_asks_only_for_what_is_left() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .respond_with(heavy_before(12, 12, 5000, thread))
            .mount(&server)
            .await;
        let service = stand_in(&server).await;

        let collected = service
            .collect_review_threads(&seven(&service), &token())
            .await
            .unwrap();

        assert!(!collected.complete);
        assert_eq!(collected.items.len(), MAXIMUM_THREADS);
        let mut expected = [100, 50, 25, 12, 24, 48, 96]
            .map(|first| json!(first))
            .to_vec();
        expected.extend(vec![json!(100); 8]);
        expected.push(json!(20));
        assert_eq!(firsts(&server.received_requests().await.unwrap()), expected);
    }

    #[tokio::test]
    async fn the_page_after_a_halved_one_asks_for_twice_as_many_up_to_a_hundred() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .respond_with(heavy_before(12, 12, 300, thread))
            .expect(9)
            .mount(&server)
            .await;
        let service = stand_in(&server).await;

        let collected = service
            .collect_review_threads(&seven(&service), &token())
            .await
            .unwrap();

        assert!(collected.complete);
        assert_eq!(collected.items.len(), 300);
        assert_eq!(collected.items[299].id, "PRRT_299");
        assert_eq!(
            firsts(&server.received_requests().await.unwrap()),
            [100, 50, 25, 12, 24, 48, 96, 100, 100].map(|first| json!(first)),
            "pages past the heavy thread double back to a hundred and no further"
        );
    }

    fn ladder_to_the_request_cap() -> Vec<Value> {
        let mut expected = [100, 50, 25, 12, 6, 3, 1]
            .map(|first| json!(first))
            .to_vec();
        while expected.len() < MAXIMUM_REQUESTS {
            expected.push(json!(2));
            expected.push(json!(1));
        }
        expected.truncate(MAXIMUM_REQUESTS);
        expected
    }

    fn read_at_one(sizes: &[Value]) -> usize {
        sizes.iter().filter(|first| **first == json!(1)).count()
    }

    #[tokio::test]
    async fn a_walk_with_a_heavy_thread_on_every_page_stops_at_the_request_cap() {
        assert_eq!(MAXIMUM_REQUESTS, 70, "the thread reads document seventy");
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .respond_with(halving_above(1, 5000, thread))
            .expect(u64::try_from(MAXIMUM_REQUESTS).unwrap())
            .mount(&server)
            .await;
        let service = stand_in(&server).await;

        let collected = service
            .collect_review_threads(&seven(&service), &token())
            .await
            .unwrap();

        let expected = ladder_to_the_request_cap();
        assert_eq!(
            expected.last(),
            Some(&json!(2)),
            "the cap falls while a page is being halved"
        );
        assert!(!collected.complete);
        assert_eq!(collected.items.len(), read_at_one(&expected));
        assert_eq!(
            collected.items.last().map(|thread| thread.id.as_str()),
            Some("PRRT_31")
        );
        assert_eq!(firsts(&server.received_requests().await.unwrap()), expected);
    }

    #[tokio::test]
    async fn counting_a_walk_with_a_heavy_thread_on_every_page_stops_at_the_request_cap() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .respond_with(halving_above(1, 5000, |id| state(id, false)))
            .expect(u64::try_from(MAXIMUM_REQUESTS).unwrap())
            .mount(&server)
            .await;
        let service = stand_in(&server).await;

        let count = service
            .fetch_review_thread_counts(&seven(&service), &token())
            .await
            .unwrap();

        let read = read_at_one(&ladder_to_the_request_cap());
        assert_eq!(
            count,
            ReviewThreadCount::new(u32::try_from(read).unwrap(), 0, false)
        );
    }

    #[tokio::test]
    async fn counting_a_walk_halved_to_fifty_stops_at_a_thousand_threads() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/graphql"))
            .respond_with(halving_above(50, 5000, |id| state(id, false)))
            .expect(39)
            .mount(&server)
            .await;
        let service = stand_in(&server).await;

        let count = service
            .fetch_review_thread_counts(&seven(&service), &token())
            .await
            .unwrap();

        assert_eq!(
            count,
            ReviewThreadCount::new(u32::try_from(MAXIMUM_THREADS).unwrap(), 0, false)
        );
    }
}
