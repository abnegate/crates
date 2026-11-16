use super::*;
use crate::commit_sha::CommitSha;
use crate::pull_request::CheckRun;
use crate::pull_request::Collected;
use crate::pull_request::CommitStatus;
use crate::pull_request::EvidenceSlot;
use crate::pull_request::HeadChecks;
use crate::pull_request::IssueComment;
use crate::pull_request::ListedPullRequest;
use crate::pull_request::MergeableState;
use crate::pull_request::OpenPullRequestPage;
use crate::pull_request::PullRequestEvidence;
use crate::pull_request::PullRequestState;
use crate::pull_request::RepositoryId;
use crate::pull_request::ReviewThreadCount;
use crate::pull_request::WorkflowRun;
use crate::pull_request::graphql_actor::GraphQlActor;
use crate::pull_request::graphql_check_run::GraphQlCheckRun;
use crate::pull_request::graphql_check_suite::GraphQlCheckSuite;
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
checkSuites(first:50){pageInfo{hasNextPage} nodes{status conclusion createdAt branch{name} repository{id} app{slug} \
workflowRun{databaseId event url createdAt updatedAt workflow{databaseId name} file{path}} \
checkRuns(first:100,filterBy:{checkType:LATEST}){pageInfo{hasNextPage} \
nodes{databaseId name status conclusion url detailsUrl startedAt completedAt}}}}}}}}}}";

/// Times a request GitHub found too large or too slow is asked again at half
/// its size.
const HALVINGS: usize = 2;

/// Fewest pull requests or rows a request has to ask for to be halved.
const HALVABLE: usize = 2;

/// The kind GitHub's GraphQL API names a query that ran out of time with.
const TIMEOUT: &str = "TIMEOUT";

/// What GitHub's GraphQL API says, in any case, of a query that ran out of
/// time when it names no kind.
const TIMEOUT_WORDS: &[&str] = &["timeout", "timed out"];

/// The state of a status context that is required but has never reported,
/// which GitHub's REST API never lists.
const EXPECTED: &str = "EXPECTED";

/// The state GitHub's GraphQL API names an open pull request with.
const OPEN_STATE: &str = "OPEN";

/// The states GitHub's GraphQL API names a pull request with that its REST
/// API calls closed, a merged one among them.
const CLOSED_STATES: &[&str] = &["CLOSED", "MERGED"];

/// The event GitHub names a run of a workflow it defines itself with, such
/// as code scanning's default setup, rather than one in the repository.
const DYNAMIC: &str = "dynamic";

/// The App whose check suites are GitHub Actions workflow runs.
const ACTIONS: &str = "github-actions";

/// Where in a query for open pull requests GitHub reports the repository it
/// could not find.
const REPOSITORY_PATH: &[&str] = &["repository"];

/// The field an evidence query lists its pull requests under, which every
/// error GitHub reports at one of them names first in its path.
const NODES: &str = "nodes";

/// The login GitHub's REST API names an account since deleted with, where
/// its GraphQL API names no author at all.
const GHOST: &str = "ghost";

/// What a page that says more open pull requests follow, without a cursor
/// that moved on, is reported as.
const NO_CURSOR: &str =
    "GitHub said more open pull requests follow but named no cursor to read them after";

impl PullRequestService {
    /// Most pull requests [`Self::fetch_pull_request_evidence`] reads in one
    /// call: about five seconds and five points of GitHub's GraphQL budget a
    /// request, well inside its ten-second limit.
    pub const EVIDENCE_BATCH: usize = 10;

    /// The page of `repository`'s open pull requests that follows the cursor
    /// `after`, at `first` a page up to GitHub's hundred, the most recently
    /// updated first, as [`Self::list_pull_requests`] orders them, with the
    /// repository's node id. One GraphQL request.
    ///
    /// Each row reads as [`Self::list_pull_requests`] reads it from REST: an
    /// App's login ends in `[bot]`, GitHub's Copilot coding agent is
    /// `Copilot`, and an account since deleted is `ghost`.
    ///
    /// [`OpenPullRequestPage::next`] is the cursor to pass for the page that
    /// follows, and `None` once GitHub says this page was the last. A page
    /// whose answer is larger than the 16 MiB this crate reads, that GitHub's
    /// gateway answers with a 502 or 504, or that GitHub says ran out of
    /// time, is asked for again at half as many rows, twice at most, so the
    /// page can hold fewer than `first` while `next` still names the next; a
    /// page that still fails is that failure.
    ///
    /// A spent rate limit is [`PullRequestError::RateLimited`], and a
    /// repository GitHub cannot find, or will not show the token, is
    /// [`PullRequestError::RepositoryNotFound`]. A row this crate cannot read
    /// is [`PullRequestError::GitHubApi`], and one whose branch or head
    /// commit it does not accept is [`PullRequestError::Parse`], never a page
    /// without it. A page that says another follows without a cursor that
    /// moved on from `after` is [`PullRequestError::GitHubApi`] too, never a
    /// page passed off as the last. GitHub Enterprise Server releases whose
    /// schema lacks a field asked for answer with
    /// [`PullRequestError::GitHubApi`]; [`Self::list_pull_requests`] reads the
    /// same rows through REST.
    pub async fn list_open_pull_requests(
        &self,
        repository: &Repository,
        token: &SecretValue,
        first: NonZeroU8,
        after: Option<&str>,
    ) -> PullRequestResult<OpenPullRequestPage> {
        let mut rows = asked(first);
        let mut halvings = 0;
        loop {
            match self.open_page(repository, token, rows, after).await {
                Err(failure) if halvable(&failure) && splits(rows, halvings) => {
                    rows /= 2;
                    halvings += 1;
                }
                read => return read,
            }
        }
    }

    /// What says whether each pull request `ids` names in `repository` is
    /// ready to merge, one [`EvidenceSlot`] for each id, in the order asked:
    /// what stands in its way, how many of its review threads are open, its
    /// newest conversation comments and what checked its head. Up to
    /// [`Self::EVIDENCE_BATCH`] pull requests are read in one GraphQL request;
    /// more is [`PullRequestError::BatchTooLarge`], unsent, and none sends
    /// nothing.
    ///
    /// `repository_id` is the node id [`Self::list_open_pull_requests`] read
    /// for `repository`, which a rename or transfer leaves as it is. A pull
    /// request read is held to it, and so is each check suite: one that ran
    /// in a fork is left out, as GitHub's REST API leaves it out of the
    /// repository's checks. A pull request is read whatever its state, so one
    /// closed or merged since it was listed comes back as
    /// [`PullRequestState::Closed`].
    ///
    /// A slot is [`EvidenceSlot::Read`] unless GitHub reported an error at
    /// it, sent nothing for it, answered for another pull request or
    /// repository, or sent what this crate cannot read; each of those fails
    /// only its own slot. A spent rate limit anywhere in the answer is
    /// [`PullRequestError::RateLimited`], and every other error GitHub
    /// reported, or a refusal of the request itself, fails the whole call as
    /// any GraphQL call reads it.
    ///
    /// A request whose answer is larger than the 16 MiB this crate reads,
    /// that GitHub's gateway answers with a 502 or 504, or that GitHub says
    /// ran out of time, is sent again as two of half as many pull requests,
    /// one after the other, and each half that fails so is halved again,
    /// twice at most, down to a single pull request: three split into one
    /// and two, two into two of one. The pull requests of a request that
    /// still fails that way are [`EvidenceSlot::Unanswered`], while the
    /// others keep what was read for them, so ten cost at most seven
    /// requests.
    ///
    /// Each slot read holds what GitHub's REST API holds for the same pull
    /// request: words GitHub's GraphQL API spells in capitals are in
    /// lowercase, an App's login ends in `[bot]`, GitHub's Copilot coding
    /// agent is `Copilot`, an account since deleted is `ghost`, a check run
    /// behind an Actions run links to its job, a status context still
    /// expected is left out, and each Actions run is listed once, at the
    /// status of its newest attempt; [`HeadChecks`] says what GraphQL leaves
    /// out of a run. The review thread count stops at a hundred threads, the
    /// comments at the newest hundred, the check suites at fifty and each
    /// suite's check runs at a hundred, and a list cut short says so in
    /// [`Collected::complete`] or [`ReviewThreadCount::complete`]. So do the
    /// workflow runs when GitHub Actions ran a suite GraphQL names no run
    /// for. A head is `None` when the newest commit GitHub listed is not the
    /// head commit it named.
    pub async fn fetch_pull_request_evidence<Id: AsRef<str>>(
        &self,
        repository: &Repository,
        token: &SecretValue,
        repository_id: &RepositoryId,
        ids: &[Id],
    ) -> PullRequestResult<Vec<EvidenceSlot>> {
        if ids.len() > Self::EVIDENCE_BATCH {
            return Err(PullRequestError::BatchTooLarge {
                asked: ids.len(),
                maximum: Self::EVIDENCE_BATCH,
            });
        }
        let ids: Vec<&str> = ids.iter().map(AsRef::as_ref).collect();
        let mut slots: Vec<Option<EvidenceSlot>> = ids.iter().map(|_| None).collect();
        let mut pending: Vec<(usize, usize, usize)> = vec![(0, ids.len(), 0)];
        while let Some((start, end, halvings)) = pending.pop() {
            let batch = &ids[start..end];
            if batch.is_empty() {
                continue;
            }
            match self
                .evidence_once(repository, token, repository_id, batch)
                .await
            {
                Ok(read) => {
                    for (slot, read) in slots[start..end].iter_mut().zip(read) {
                        *slot = Some(read);
                    }
                }
                Err(failure) if halvable(&failure) && splits(batch.len(), halvings) => {
                    let middle = start + batch.len() / 2;
                    pending.push((middle, end, halvings + 1));
                    pending.push((start, middle, halvings + 1));
                }
                Err(failure) if halvable(&failure) => {
                    for slot in &mut slots[start..end] {
                        *slot = Some(EvidenceSlot::Unanswered);
                    }
                }
                Err(failure) => return Err(failure),
            }
        }

        let slots: Vec<EvidenceSlot> = slots
            .into_iter()
            .map(|slot| slot.unwrap_or(EvidenceSlot::Unanswered))
            .collect();
        for (index, slot) in slots.iter().enumerate() {
            if let Some(reason) = slot.reason() {
                tracing::debug!(
                    slot = index,
                    reason,
                    "Read no evidence for a pull request asked for"
                );
            }
        }
        Ok(slots)
    }

    /// One request for the evidence on `ids`.
    async fn evidence_once(
        &self,
        repository: &Repository,
        token: &SecretValue,
        repository_id: &RepositoryId,
        ids: &[&str],
    ) -> PullRequestResult<Vec<EvidenceSlot>> {
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
                true => EvidenceSlot::Reported,
                false => parse_node(repository, repository_id, id, node),
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
    ) -> PullRequestResult<OpenPullRequestPage> {
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
        if reported(&errors, RATE_LIMITED) {
            return Err(PullRequestError::RateLimited);
        }
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
        let items = page
            .nodes
            .into_iter()
            .map(|row| listed(repository, row))
            .collect::<PullRequestResult<Vec<ListedPullRequest>>>()?;

        Ok(OpenPullRequestPage::new(
            RepositoryId::new(open.id),
            items,
            next,
        ))
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
/// repository GitHub could not find first, and every other refusal as any
/// GraphQL call reads it.
fn open_rejection(errors: &[GraphQlError]) -> PullRequestError {
    match missing(errors, REPOSITORY_PATH) {
        true => PullRequestError::RepositoryNotFound,
        false => rejection(errors),
    }
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

/// What `node` holds for the pull request `id` names, in the repository
/// `repository_id` names.
fn parse_node(
    repository: &Repository,
    repository_id: &RepositoryId,
    id: &str,
    node: Value,
) -> EvidenceSlot {
    if node.is_null() {
        return EvidenceSlot::Missing;
    }
    let Ok(node) = serde_json::from_value::<GraphQlPullRequestEvidence>(node) else {
        return EvidenceSlot::Unreadable;
    };
    if node.pull_request.id != id || node.repository.id != repository_id.as_str() {
        return EvidenceSlot::Foreign;
    }
    match read_evidence(repository, repository_id, node) {
        Ok(evidence) => EvidenceSlot::Read(Box::new(evidence)),
        Err(_) => EvidenceSlot::Unreadable,
    }
}

/// A pull request with its evidence, from what GitHub answered for it.
fn read_evidence(
    repository: &Repository,
    repository_id: &RepositoryId,
    node: GraphQlPullRequestEvidence,
) -> PullRequestResult<PullRequestEvidence> {
    let pull_request = listed(repository, node.pull_request)?;
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
            pull_request.head_sha.as_ref().is_some_and(|head| {
                CommitSha::parse(&commit.oid).is_ok_and(|newest| &newest == head)
            })
        })
        .map(|commit| head_checks(repository, repository_id, &pull_request.head, commit))
        .transpose()?;
    let evidence = PullRequestEvidence::new(
        pull_request,
        merge_state(node.merge_state_status.as_deref()),
        threads,
        comments,
    );
    Ok(match head {
        Some(head) => evidence.with_head(head),
        None => evidence,
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
        state: pull_request_state(&row.state),
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

/// Whether a pull request is open, from the word GitHub's GraphQL API names
/// its state with: a merged one is closed, as GitHub's REST API says.
fn pull_request_state(state: &str) -> PullRequestState {
    if state == OPEN_STATE {
        PullRequestState::Open
    } else if CLOSED_STATES.contains(&state) {
        PullRequestState::Closed
    } else {
        PullRequestState::Unknown
    }
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

/// What checked `commit`, the head of a pull request from `head`, in the
/// repository `repository_id` names: the runs of its suites there, newest
/// first and each once, the latest check runs of those suites, and every
/// status but one still expected.
fn head_checks(
    repository: &Repository,
    repository_id: &RepositoryId,
    head: &BranchName,
    commit: GraphQlCheckedCommit,
) -> PullRequestResult<HeadChecks> {
    let sha = CommitSha::parse(&commit.oid)?;
    let suites_complete = last(commit.check_suites.page_info.as_ref());
    let mut runs_complete = suites_complete;
    let mut check_runs_complete = suites_complete;
    let mut check_runs: Vec<CheckRun> = Vec::new();
    let mut runs: Vec<(WorkflowRun, String)> = Vec::new();

    for suite in commit
        .check_suites
        .nodes
        .into_iter()
        .filter(|suite| suite.repository.id == repository_id.as_str())
    {
        runs_complete &= !runless_actions(&suite);
        let actions = suite.workflow_run.is_some();
        match suite.check_runs {
            Some(page) => {
                check_runs_complete &= last(page.page_info.as_ref());
                check_runs.extend(
                    page.nodes
                        .into_iter()
                        .map(|run| check_run(run, &sha, actions)),
                );
            }
            None => check_runs_complete = false,
        }
        if let Some(run) = suite.workflow_run {
            let branch = suite
                .branch
                .map(|branch| branch.name)
                .or_else(|| (run.event != DYNAMIC).then(|| head.to_string()));
            let read = workflow_run(
                repository,
                &sha,
                run,
                &suite.status,
                suite.conclusion,
                branch,
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
            runs_complete,
        ),
        Collected::new(check_runs, check_runs_complete),
        Collected::new(statuses, true),
    ))
}

/// Whether `suite` is one GitHub Actions ran but GraphQL names no run for,
/// as it may while the run is being set up, so a run REST lists is missing.
fn runless_actions(suite: &GraphQlCheckSuite) -> bool {
    suite.workflow_run.is_none()
        && suite
            .app
            .as_ref()
            .and_then(|app| app.slug.as_deref())
            .is_some_and(|slug| slug == ACTIONS)
}

/// The Actions run `run`, on `sha` in `repository` from `branch` when it is
/// known, at the `status` and `conclusion` of the suite it ran as, in
/// lowercase as GitHub's REST API spells them.
fn workflow_run(
    repository: &Repository,
    sha: &CommitSha,
    run: GraphQlWorkflowRun,
    status: &str,
    conclusion: Option<String>,
    branch: Option<String>,
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
        Some(branch) => read.with_head_branch(branch),
        None => read,
    }
}

/// A check run on `sha`, its words in lowercase as GitHub's REST API spells
/// them, read at its Actions job when an Actions run is behind it, as GitHub's
/// REST API links it.
fn check_run(run: GraphQlCheckRun, sha: &CommitSha, actions: bool) -> CheckRun {
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
