use super::*;
use crate::commit_sha::CommitSha;
use crate::pull_request::CheckRun;
use crate::pull_request::Collected;
use crate::pull_request::CommitStatus;
use crate::pull_request::HeadChecks;
use crate::pull_request::IssueComment;
use crate::pull_request::ListedPullRequest;
use crate::pull_request::MergeableState;
use crate::pull_request::OpenPage;
use crate::pull_request::OpenPullRequest;
use crate::pull_request::PullRequestState;
use crate::pull_request::ReviewThreadCount;
use crate::pull_request::WorkflowRun;
use crate::pull_request::graphql_actor::GraphQlActor;
use crate::pull_request::graphql_branch::GraphQlBranch;
use crate::pull_request::graphql_check_run::GraphQlCheckRun;
use crate::pull_request::graphql_checked_commit::GraphQlCheckedCommit;
use crate::pull_request::graphql_error::GraphQlError;
use crate::pull_request::graphql_issue_comment::GraphQlIssueComment;
use crate::pull_request::graphql_listed_pull_request::GraphQlListedPullRequest;
use crate::pull_request::graphql_nodes::GraphQlNodes;
use crate::pull_request::graphql_open_repository::GraphQlOpenRepository;
use crate::pull_request::graphql_pull_request_evidence::GraphQlPullRequestEvidence;
use crate::pull_request::graphql_query::GraphQlQuery;
use crate::pull_request::graphql_response::GraphQlResponse;
use crate::pull_request::graphql_status_context::GraphQlStatusContext;
use crate::pull_request::graphql_workflow_run::GraphQlWorkflowRun;
use crate::pull_request::service::graphql::NO_DATA;
use crate::pull_request::service::graphql::RATE_LIMITED;
use crate::pull_request::service::graphql::rejection;
use crate::pull_request::service::graphql::reported;
use crate::pull_request::service::threads::last;
use crate::pull_request::service::threads::missing;
use crate::pull_request::service::threads::next_cursor;
use serde_json::Value;
use serde_json::json;
use std::collections::HashSet;

#[cfg(test)]
mod parity_tests;
#[cfg(test)]
mod tests;

/// Reads the page of `first` of a repository's open pull requests that
/// follows the cursor `after`, the most recently updated first, and the
/// repository's node id.
const OPEN: &str = "query($owner:String!,$name:String!,$first:Int!,$after:String){\
repository(owner:$owner,name:$name){id \
pullRequests(states:OPEN,first:$first,after:$after,orderBy:{field:UPDATED_AT,direction:DESC}){\
pageInfo{hasNextPage endCursor} \
nodes{id number title url state isDraft updatedAt headRefName headRefOid baseRefName \
author{__typename login}}}}}";

/// Reads each pull request its `ids` name with what stands in its way, its
/// first hundred review threads, its newest hundred conversation comments,
/// and the combined status, check suites, Actions runs and latest check runs
/// on its newest commit.
const EVIDENCE: &str = "query($ids:[ID!]!){nodes(ids:$ids){... on PullRequest{\
id number title url state isDraft updatedAt headRefName headRefOid baseRefName \
repository{id} \
author{__typename login} \
mergeStateStatus \
reviewThreads(first:100){pageInfo{hasNextPage} nodes{isResolved}} \
comments(last:100){totalCount nodes{fullDatabaseId body url createdAt author{__typename login}}} \
commits(last:1){nodes{commit{oid \
status{contexts{context state description targetUrl createdAt}} \
checkSuites(first:50){pageInfo{hasNextPage} nodes{status conclusion createdAt branch{name} repository{id} \
workflowRun{databaseId event url createdAt updatedAt workflow{databaseId name} file{path}} \
checkRuns(first:100,filterBy:{checkType:LATEST}){pageInfo{hasNextPage} \
nodes{databaseId name status conclusion url detailsUrl startedAt completedAt}}}}}}}}}}";

/// Pull requests one evidence request asks for: about five seconds and five
/// points of GitHub's GraphQL budget, well inside its ten-second limit.
const EVIDENCE_BATCH: usize = 10;

/// Times a request GitHub found too large or too slow is asked again at half
/// its size before its failure is the answer.
const HALVINGS: usize = 2;

/// Fewest pull requests a request has to ask for to be halved.
const HALVABLE: usize = 3;

/// The statuses GitHub's gateway answers a GraphQL query that ran out of
/// time with.
const GATEWAY_TIMEOUTS: &[u16] = &[502, 504];

/// The kind GitHub's GraphQL API names a query that ran out of time with.
const TIMEOUT: &str = "TIMEOUT";

/// What GitHub's GraphQL API says, in any case, of a query that ran out of
/// time when it names no kind.
const TIMEOUT_WORDS: &[&str] = &["timeout", "timed out"];

/// The state of a status context that is required but has never reported,
/// which GitHub's REST API never lists.
const EXPECTED: &str = "EXPECTED";

/// Where in a query for open pull requests GitHub reports the repository it
/// could not find.
const REPOSITORY_PATH: &[&str] = &["repository"];

/// The field an evidence query lists its pull requests under, which every
/// error GitHub reports at one of them names first in its path.
const NODES: &str = "nodes";

/// The login GitHub's REST API names an account since deleted with, where
/// its GraphQL API names no author at all.
const GHOST: &str = "ghost";

/// What a query GitHub's GraphQL API ran out of time answering is reported
/// as once halving it no further helps.
const TIMED_OUT: &str = "GitHub's GraphQL API ran out of time answering";

/// What a page that says more open pull requests follow, without a cursor
/// that moved on, is reported as.
const NO_CURSOR: &str =
    "GitHub said more open pull requests follow but named no cursor to read them after";

impl PullRequestService {
    /// The page of `repository`'s open pull requests that follows the cursor
    /// `after`, at `first` a page up to GitHub's hundred, the most recently
    /// updated first, as [`Self::list_pull_requests`] orders them, with the
    /// repository's node id. One GraphQL request.
    ///
    /// Each row reads as [`Self::list_pull_requests`] reads it from REST: an
    /// App's login ends in `[bot]`, and an account since deleted is `ghost`.
    ///
    /// [`OpenPage::after`] is the cursor to pass for the page that follows,
    /// and `None` once GitHub says this page was the last. A page whose answer
    /// is larger than the 16 MiB this crate reads, that GitHub's gateway
    /// answers with a 502 or 504, or that GitHub says ran out of time, is
    /// asked for again at half as many rows, twice at most, so the page can
    /// hold fewer than `first` while `after` still names the next; a page that
    /// still fails is that failure.
    ///
    /// A repository GitHub cannot find, or will not show the token, is
    /// [`PullRequestError::RepositoryNotFound`]. A row this crate cannot read
    /// is [`PullRequestError::GitHubApi`], and one whose branch or head
    /// commit it does not accept is [`PullRequestError::Parse`], never a page
    /// without it. A page that says another follows without a cursor that
    /// moved on from `after` is [`PullRequestError::GitHubApi`] too, never a
    /// page passed off as the last. GitHub Enterprise Server releases whose
    /// schema lacks a field asked for answer with
    /// [`PullRequestError::GitHubApi`]; [`Self::list_pull_requests`] reads the
    /// same rows through REST.
    pub async fn fetch_open_pull_requests(
        &self,
        repository: &Repository,
        token: &SecretValue,
        first: NonZeroU8,
        after: Option<&str>,
    ) -> PullRequestResult<OpenPage> {
        let mut rows = asked(first);
        let mut halvings = 0;
        loop {
            match self.open_page(repository, token, rows, after).await {
                Err(failure) if halves(&failure) && splits(rows, halvings) => {
                    rows /= 2;
                    halvings += 1;
                }
                read => return read,
            }
        }
    }

    /// Each open pull request `ids` names in `repository`, in the order
    /// asked, with what stands in its way, how many of its review threads are
    /// open, its newest conversation comments and what checked its head, read
    /// ten to a GraphQL request.
    ///
    /// `repository_id` is the node id [`Self::fetch_open_pull_requests`]
    /// read for `repository`, which a rename or transfer leaves as it is. A
    /// pull request read is held to it, and so is each check suite: one that
    /// ran in a fork is left out, as GitHub's REST API leaves it out of the
    /// repository's checks.
    ///
    /// An entry is `None`, without failing the others, when GitHub reported
    /// an error at it, sent null for it, named another pull request or one in
    /// another repository, or sent one this crate cannot read: a caller reads
    /// that pull request another way. Any other error GitHub reported fails
    /// the whole read, as any GraphQL call reads it, a spent rate limit as
    /// [`PullRequestError::RateLimited`].
    ///
    /// A request whose answer is larger than the 16 MiB this crate reads,
    /// that GitHub's gateway answers with a 502 or 504, or that GitHub says
    /// ran out of time, is sent again as two of half as many pull requests
    /// each, one after the other; a half is halved again once at most, and
    /// never below two pull requests, so ten pull requests cost at most seven
    /// requests. A request that still fails is that failure, for every pull
    /// request asked: [`PullRequestError::GitHubApi`] for a query that ran out
    /// of time, and [`PullRequestError::Refused`] for a gateway's status.
    ///
    /// Each entry reads as GitHub's REST API reads the same pull request:
    /// words GitHub's GraphQL API spells in capitals are in lowercase, an
    /// App's login ends in `[bot]`, an account since deleted is `ghost`, a
    /// check run behind an Actions run links to its job, a status context
    /// still expected is left out, and each Actions run is listed once, at
    /// the status of its newest attempt.
    ///
    /// What each entry holds, and when a list in it is not every row, is
    /// [`OpenPullRequest`]'s to say: the review thread count stops at a
    /// hundred threads, the comments at the newest hundred, the check suites
    /// at fifty and each suite's check runs at a hundred, and a list cut short
    /// says so in [`Collected::complete`] or [`ReviewThreadCount::complete`].
    /// A head is `None` when the newest commit GitHub listed is not the head
    /// commit it named.
    pub async fn fetch_pull_request_evidence(
        &self,
        repository: &Repository,
        token: &SecretValue,
        repository_id: &str,
        ids: &[String],
    ) -> PullRequestResult<Vec<Option<OpenPullRequest>>> {
        let mut evidence = Vec::with_capacity(ids.len());
        for batch in ids.chunks(EVIDENCE_BATCH) {
            evidence.extend(
                self.evidence_batch(repository, token, repository_id, batch)
                    .await?,
            );
        }
        Ok(evidence)
    }

    /// The evidence for `ids`, asked for again in halves while GitHub finds
    /// the request too large or too slow.
    async fn evidence_batch(
        &self,
        repository: &Repository,
        token: &SecretValue,
        repository_id: &str,
        ids: &[String],
    ) -> PullRequestResult<Vec<Option<OpenPullRequest>>> {
        let mut evidence = Vec::with_capacity(ids.len());
        let mut pending: Vec<(&[String], usize)> = vec![(ids, 0)];
        while let Some((batch, halvings)) = pending.pop() {
            match self
                .evidence_once(repository, token, repository_id, batch)
                .await
            {
                Ok(read) => evidence.extend(read),
                Err(failure) if halves(&failure) && splits(batch.len(), halvings) => {
                    let (front, back) = batch.split_at(batch.len() / 2);
                    pending.push((back, halvings + 1));
                    pending.push((front, halvings + 1));
                }
                Err(failure) => return Err(failure),
            }
        }
        Ok(evidence)
    }

    /// One request for the evidence on `ids`.
    async fn evidence_once(
        &self,
        repository: &Repository,
        token: &SecretValue,
        repository_id: &str,
        ids: &[String],
    ) -> PullRequestResult<Vec<Option<OpenPullRequest>>> {
        let GraphQlResponse { data, errors } =
            self.answer(token, EVIDENCE, &json!({ "ids": ids })).await?;
        if reported(&errors, RATE_LIMITED) {
            return Err(PullRequestError::RateLimited);
        }
        let data = answered_with(data).ok_or_else(|| unanswered(&errors, rejection))?;
        let (slotted, general): (Vec<GraphQlError>, Vec<GraphQlError>) =
            errors.into_iter().partition(|error| slot(error).is_some());
        if !general.is_empty() {
            return Err(rejection(&general));
        }
        let failed: HashSet<usize> = slotted.iter().filter_map(slot).collect();
        let answered: GraphQlNodes = serde_json::from_value(data).map_err(|_| unreadable())?;
        if answered.nodes.len() != ids.len() {
            return Err(unreadable());
        }

        Ok(answered
            .nodes
            .into_iter()
            .zip(ids)
            .enumerate()
            .map(|(index, (node, id))| match failed.contains(&index) {
                true => None,
                false => evidence(repository, repository_id, id, node),
            })
            .collect())
    }

    /// One request for `rows` of `repository`'s open pull requests after the
    /// cursor `after`.
    async fn open_page(
        &self,
        repository: &Repository,
        token: &SecretValue,
        rows: usize,
        after: Option<&str>,
    ) -> PullRequestResult<OpenPage> {
        let GraphQlResponse { data, errors } = self
            .answer(
                token,
                OPEN,
                &json!({
                    "owner": repository.owner(),
                    "name": repository.name(),
                    "first": rows,
                    "after": after,
                }),
            )
            .await?;
        let data = answered_with(data).ok_or_else(|| unanswered(&errors, open_rejection))?;
        if !errors.is_empty() {
            return Err(open_rejection(&errors));
        }
        let answer: GraphQlQuery<GraphQlOpenRepository> =
            serde_json::from_value(data).map_err(|_| unreadable())?;
        let open = answer
            .repository
            .ok_or(PullRequestError::RepositoryNotFound)?;
        let page = open.pull_requests;
        let next = match page.page_info.as_ref().and_then(|info| info.has_next_page) {
            Some(false) => None,
            Some(true) => Some(
                next_cursor(page.page_info, after)
                    .ok_or_else(|| PullRequestError::GitHubApi(NO_CURSOR.to_string()))?,
            ),
            None => return Err(unreadable()),
        };
        let pull_requests = page
            .nodes
            .into_iter()
            .map(|row| listed(repository, row))
            .collect::<PullRequestResult<Vec<ListedPullRequest>>>()?;

        Ok(OpenPage::new(open.id, pull_requests, next))
    }
}

/// Whether a request that failed with `failure` may be asked again smaller:
/// an answer too large to read, a gateway's timeout, or GitHub saying the
/// query ran out of time.
fn halves(failure: &PullRequestError) -> bool {
    match failure {
        PullRequestError::GitHubApi(message) => message == OVERSIZED || message == TIMED_OUT,
        PullRequestError::Refused { status, .. } => GATEWAY_TIMEOUTS.contains(status),
        _ => false,
    }
}

/// Whether a request for `size` rows or pull requests, already halved
/// `halvings` times, may be halved again.
fn splits(size: usize, halvings: usize) -> bool {
    size >= HALVABLE && halvings < HALVINGS
}

/// The data GitHub answered with, unless it sent none or sent null.
fn answered_with(data: Option<Value>) -> Option<Value> {
    data.filter(|data| !data.is_null())
}

/// What an answer that carried no data means: a query that ran out of time,
/// the refusal `refusal` names for the errors GitHub reported, or, with
/// none, an answer with no data.
fn unanswered(
    errors: &[GraphQlError],
    refusal: fn(&[GraphQlError]) -> PullRequestError,
) -> PullRequestError {
    if errors.iter().any(timed_out) {
        return PullRequestError::GitHubApi(TIMED_OUT.to_string());
    }
    match errors.is_empty() {
        true => PullRequestError::GitHubApi(NO_DATA.to_string()),
        false => refusal(errors),
    }
}

/// Whether GitHub's GraphQL API said a query ran out of time, by its kind or
/// in its words.
fn timed_out(error: &GraphQlError) -> bool {
    let message = error.message.to_ascii_lowercase();
    error.kind.as_deref() == Some(TIMEOUT)
        || TIMEOUT_WORDS.iter().any(|words| message.contains(words))
}

/// The refusal errors reported for a page of open pull requests name: a
/// spent rate limit first, then a repository GitHub could not find, and
/// every other refusal as any GraphQL call reads it.
fn open_rejection(errors: &[GraphQlError]) -> PullRequestError {
    if reported(errors, RATE_LIMITED) {
        return PullRequestError::RateLimited;
    }
    if missing(errors, REPOSITORY_PATH) {
        return PullRequestError::RepositoryNotFound;
    }
    rejection(errors)
}

/// The entry of a list of node ids an error was reported at: its path starts
/// with `nodes` and that entry's index.
fn slot(error: &GraphQlError) -> Option<usize> {
    match error.path.as_deref()? {
        [nodes, index, ..] if nodes.as_str() == Some(NODES) => {
            usize::try_from(index.as_u64()?).ok()
        }
        _ => None,
    }
}

/// An answer this crate cannot read.
fn unreadable() -> PullRequestError {
    PullRequestError::GitHubApi(UNREADABLE.to_string())
}

/// The pull request `node` holds, when it is the one `id` names, in the
/// repository `repository_id` names, and can be read whole.
fn evidence(
    repository: &Repository,
    repository_id: &str,
    id: &str,
    node: Value,
) -> Option<OpenPullRequest> {
    let node: GraphQlPullRequestEvidence = serde_json::from_value(node).ok()?;
    if node.listed.id != id || node.repository.id != repository_id {
        return None;
    }
    assembled(repository, repository_id, node).ok()
}

/// A pull request with its evidence, from what GitHub answered for it.
fn assembled(
    repository: &Repository,
    repository_id: &str,
    node: GraphQlPullRequestEvidence,
) -> PullRequestResult<OpenPullRequest> {
    let listed = listed(repository, node.listed)?;
    let threads = node.review_threads.nodes.iter().fold(
        ReviewThreadCount::new(0, 0, last(node.review_threads.page_info.as_ref())),
        |count, thread| count.with(thread.is_resolved),
    );
    let read = u64::try_from(node.comments.nodes.len()).unwrap_or(u64::MAX);
    let comments = Collected::new(
        node.comments.nodes.into_iter().map(comment).collect(),
        node.comments.total_count <= read,
    );
    let head = node
        .commits
        .nodes
        .into_iter()
        .next_back()
        .map(|newest| newest.commit)
        .filter(|commit| {
            listed.head_sha.as_ref().is_some_and(|head| {
                CommitSha::parse(&commit.oid).is_ok_and(|newest| &newest == head)
            })
        })
        .map(|commit| head_checks(repository, repository_id, commit))
        .transpose()?;
    let opened = OpenPullRequest::new(
        listed,
        merge_state(node.merge_state_status.as_deref()),
        threads,
        comments,
    );
    Ok(match head {
        Some(head) => opened.with_head(head),
        None => opened,
    })
}

/// A pull request as a page of open pull requests lists it, its branches and
/// head commit parsed.
fn listed(
    repository: &Repository,
    row: GraphQlListedPullRequest,
) -> PullRequestResult<ListedPullRequest> {
    let number = NonZeroU64::new(row.number).ok_or_else(unreadable)?;
    Ok(ListedPullRequest {
        reference: PullRequestReference::new(repository.clone(), number),
        node_id: row.id,
        title: row.title,
        url: row.url,
        state: lowercase_as(&row.state).unwrap_or(PullRequestState::Unknown),
        draft: row.is_draft,
        head: BranchName::parse(&row.head_ref_name)?,
        base: BranchName::parse(&row.base_ref_name)?,
        head_sha: row
            .head_ref_oid
            .as_deref()
            .map(CommitSha::parse)
            .transpose()?,
        updated_at: row.updated_at,
        author_login: login(row.author),
    })
}

/// What stands in a pull request's way, from the word GitHub's GraphQL API
/// spells in capitals and its REST API in lowercase.
fn merge_state(status: Option<&str>) -> MergeableState {
    status.and_then(lowercase_as).unwrap_or_default()
}

/// A word GitHub's GraphQL API spells in capitals, read as its REST API
/// spells it in lowercase.
fn lowercase_as<T: DeserializeOwned>(word: &str) -> Option<T> {
    serde_json::from_value(Value::String(word.to_ascii_lowercase())).ok()
}

/// The login of `author` as GitHub's REST API spells it, `ghost` for an
/// account since deleted, which its GraphQL API sends as no author.
fn login(author: Option<GraphQlActor>) -> Option<String> {
    match author {
        Some(author) => author.login_as_listed(),
        None => Some(GHOST.to_string()),
    }
}

/// A conversation comment, its author named as GitHub's REST API names them.
fn comment(written: GraphQlIssueComment) -> IssueComment {
    IssueComment::new(
        written.full_database_id.0,
        login(written.author).unwrap_or_default(),
        written.body.unwrap_or_default(),
        written.url,
        written.created_at,
    )
}

/// What checked `commit` in the repository `repository_id` names: the runs
/// of its suites there, newest first and each once, the latest check runs of
/// those suites, and every status but one still expected.
fn head_checks(
    repository: &Repository,
    repository_id: &str,
    commit: GraphQlCheckedCommit,
) -> PullRequestResult<HeadChecks> {
    let sha = CommitSha::parse(&commit.oid)?;
    let suites_complete = last(commit.check_suites.page_info.as_ref());
    let mut check_runs_complete = suites_complete;
    let mut check_runs: Vec<CheckRun> = Vec::new();
    let mut runs: Vec<(WorkflowRun, String)> = Vec::new();

    for suite in commit
        .check_suites
        .nodes
        .into_iter()
        .filter(|suite| suite.repository.id == repository_id)
    {
        let actions = suite.workflow_run.is_some();
        match suite.check_runs {
            Some(page) => {
                check_runs_complete &= last(page.page_info.as_ref());
                check_runs.extend(
                    page.nodes
                        .into_iter()
                        .map(|check_run| checked(check_run, &sha, actions)),
                );
            }
            None => check_runs_complete = false,
        }
        if let Some(run) = suite.workflow_run {
            let read = workflow_run(
                repository,
                &sha,
                run,
                &suite.status,
                suite.conclusion,
                suite.branch,
            );
            runs.push((read, suite.created_at));
        }
    }

    runs.sort_by(|(left, left_created), (right, right_created)| {
        right
            .id
            .cmp(&left.id)
            .then_with(|| right_created.cmp(left_created))
    });
    runs.dedup_by_key(|(run, _)| run.id);
    let statuses = commit
        .status
        .map(|status| status.contexts)
        .unwrap_or_default()
        .into_iter()
        .filter(|context| !context.state.eq_ignore_ascii_case(EXPECTED))
        .map(status)
        .collect();

    Ok(HeadChecks::new(
        sha,
        Collected::new(
            runs.into_iter().map(|(run, _)| run).collect(),
            suites_complete,
        ),
        Collected::new(check_runs, check_runs_complete),
        Collected::new(statuses, true),
    ))
}

/// The Actions run `run`, on `sha` in `repository`, at the `status` and
/// `conclusion` of the suite it ran as and on its `branch`, in lowercase as
/// GitHub's REST API spells them.
fn workflow_run(
    repository: &Repository,
    sha: &CommitSha,
    run: GraphQlWorkflowRun,
    status: &str,
    conclusion: Option<String>,
    branch: Option<GraphQlBranch>,
) -> WorkflowRun {
    let read = WorkflowRun::new(
        run.database_id,
        run.workflow.database_id,
        run.file.map(|file| file.path).unwrap_or_default(),
        run.event,
        sha.clone(),
        run.created_at,
        run.updated_at,
        run.url,
        repository.to_string(),
    )
    .with_name(run.workflow.name)
    .with_status(status.to_ascii_lowercase());
    let read = match conclusion {
        Some(conclusion) => read.with_conclusion(conclusion.to_ascii_lowercase()),
        None => read,
    };
    match branch {
        Some(branch) => read.with_head_branch(branch.name),
        None => read,
    }
}

/// A check run on `sha`, its words in lowercase as GitHub's REST API spells
/// them, read at its Actions job when an Actions run is behind it, as GitHub's
/// REST API links it.
fn checked(run: GraphQlCheckRun, sha: &CommitSha, actions: bool) -> CheckRun {
    let url = match actions {
        true => run.details_url.clone(),
        false => run.url,
    };
    CheckRun {
        id: run.database_id,
        name: run.name,
        head_sha: Some(sha.clone()),
        status: run.status.to_ascii_lowercase(),
        conclusion: run
            .conclusion
            .map(|conclusion| conclusion.to_ascii_lowercase()),
        url,
        details_url: run.details_url,
        started_at: run.started_at,
        completed_at: run.completed_at,
    }
}

/// A status context's latest status, its state in lowercase as GitHub's REST
/// API spells it.
fn status(context: GraphQlStatusContext) -> CommitStatus {
    CommitStatus {
        context: context.context,
        state: context.state.to_ascii_lowercase(),
        description: context.description,
        target_url: context.target_url,
        created_at: context.created_at,
    }
}
