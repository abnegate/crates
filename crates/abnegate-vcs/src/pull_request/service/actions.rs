use super::*;
use crate::commit_sha::CommitSha;
use crate::pull_request::Collected;
use crate::pull_request::JobLog;
use crate::pull_request::JobLogError;
use crate::pull_request::WorkflowJob;
use crate::pull_request::WorkflowRun;
use crate::pull_request::WorkflowRunFilter;
use crate::pull_request::content_range::BYTES_UNIT;
use crate::pull_request::content_range::ContentRange;
use crate::pull_request::github_workflow_job::GitHubWorkflowJob;
use crate::pull_request::github_workflow_run::GitHubWorkflowRun;
use crate::pull_request::github_workflow_runs::GitHubWorkflowRuns;
use crate::pull_request::log_end::LogEnd;
use crate::pull_request::service::checks::walk;

/// The hosts GitHub stores job logs on, each standing for itself and every
/// host under it.
const LOG_HOSTS: &[&str] = &[
    "blob.core.windows.net",
    "pipelines.actions.githubusercontent.com",
    "objects.githubusercontent.com",
    "github.com",
];

/// The only scheme a job log is followed to off the origin.
const SECURE_SCHEME: &str = "https";

/// The bits of a UTF-8 byte that say whether it continues a character.
const CONTINUATION_MASK: u8 = 0b1100_0000;

/// Those bits in a byte that continues a character.
const CONTINUATION: u8 = 0b1000_0000;

/// Most bytes that continue one UTF-8 character.
const LONGEST_CONTINUATION: usize = 3;

impl PullRequestService {
    /// Every workflow run in `repository` that `filter` admits, newest first,
    /// as GitHub lists them, read a hundred a page until a short page, and
    /// whether that is every one.
    ///
    /// The read stops at the tenth page, so it reaches at most 1,000 runs,
    /// and GitHub lists at most 1,000 of any filtered listing. The read is
    /// [`Collected::complete`] when it read every run GitHub counted, even on
    /// a full tenth page, so exactly 1,000 counted and read is complete. It is
    /// `false`, never an error, when GitHub counted more than were read: when
    /// more than 1,000 match a filter, when an unfiltered listing holds more
    /// than 1,000, or when the walk ended short of the count, as it may when
    /// runs start while it reads. Without a count, a full tenth page is
    /// `false` too.
    ///
    /// A run without a field GitHub always sends, its identifier and its
    /// workflow's among them, or whose attempt does not fit a `u32`, is
    /// [`PullRequestError::GitHubApi`], and one whose commit is not a full
    /// commit identifier is [`PullRequestError::Parse`]: never a shorter list.
    /// A run GitHub lists twice, as it may while a run is retried during the
    /// walk, is listed twice here, and the read is then not complete: the
    /// runs moved between pages, and one may have moved past those read.
    pub async fn collect_workflow_runs(
        &self,
        repository: &Repository,
        token: &SecretValue,
        filter: &WorkflowRunFilter,
    ) -> PullRequestResult<Collected<WorkflowRun>> {
        let mut url = self.repository_endpoint(repository, &["actions", "runs"])?;
        url.query_pairs_mut().extend_pairs(filter.pairs());

        let runs = walk(
            self,
            &url,
            token,
            |page: GitHubWorkflowRuns| (page.total_count, page.workflow_runs),
            |run: &GitHubWorkflowRun| Some(run.id),
            read_as_counted,
        )
        .await?;
        let items = runs
            .items
            .into_iter()
            .map(run)
            .collect::<PullRequestResult<Vec<_>>>()?;

        Ok(Collected::new(items, runs.complete))
    }

    /// The workflow job GitHub identifies by `job` in `repository`.
    ///
    /// An answer without a field GitHub always sends, the job's identifier,
    /// run, name, status and commit, is [`PullRequestError::GitHubApi`], and
    /// one whose commit is not a full commit identifier is
    /// [`PullRequestError::Parse`]. Whether the job belongs to the run, commit
    /// or pull request a caller has in mind is the caller's to check.
    pub async fn fetch_workflow_job(
        &self,
        repository: &Repository,
        token: &SecretValue,
        job: NonZeroU64,
    ) -> PullRequestResult<WorkflowJob> {
        let job = job.to_string();
        let answered: GitHubWorkflowJob = self
            .get(
                self.repository_endpoint(repository, &["actions", "jobs", &job])?,
                token,
            )
            .await?;
        workflow_job(answered)
    }

    /// The first `limit` bytes of the log of the workflow job `job` in
    /// `repository`, and whether the log went on past them.
    ///
    /// `limit` is the caller's bound on the buffer it is handed: however long
    /// the log, no more than `limit` bytes of it are held.
    ///
    /// The request for a log carries the token, or none for an empty token
    /// under [`Self::with_anonymous_reads`]. GitHub answers it with the log
    /// itself or with a redirect to where it stores it, under an address it
    /// signs. That redirect is followed once, and without the
    /// token, only to this service's own origin or over HTTPS, on its default
    /// port, to a host GitHub stores logs on or one under it, and never to an
    /// address that carries credentials; anywhere else is
    /// [`JobLogError::UnexpectedHost`], and a redirect that names nowhere is
    /// [`JobLogError::Undirected`]. A redirect within the origin drops the
    /// token too, so the log of a private repository that was renamed, which
    /// GitHub redirects to the new name, is [`JobLogError::Refused`] with
    /// 404. Both requests together spend the one timeout a request is given;
    /// under [`Self::with_retry_after`], it runs from the attempt GitHub
    /// answered, so neither an attempt answered with a rate limit nor the
    /// wait after it spends any of it.
    ///
    /// A refusal of the request for the log is reported as any refusal of
    /// this service's is. Where the log is stored, a status other than a
    /// success, a redirect included, which is not followed, is
    /// [`JobLogError::Refused`], and a download that could not be sent, or
    /// whose time ran out before the store answered, is
    /// [`JobLogError::Unreachable`]. A log that stops arriving part way
    /// through, from either, or whose time runs out while it arrives, is
    /// [`JobLogError::Interrupted`]: never a shorter log. No error carries
    /// the signed address.
    pub async fn fetch_job_log(
        &self,
        repository: &Repository,
        token: &SecretValue,
        job: NonZeroU64,
        limit: usize,
    ) -> PullRequestResult<JobLog> {
        self.job_log(repository, token, job, limit, LogEnd::Start)
            .await
    }

    /// The last `limit` bytes of the log of the workflow job `job` in
    /// `repository`, and whether the log began before them: where a job
    /// failed, which a long log's first bytes never reach.
    ///
    /// The log is asked for, redirected, timed and refused as
    /// [`Self::fetch_job_log`] says, and errs as it does. The request to the
    /// target of GitHub's redirect asks for the last `limit` bytes alone,
    /// with `Range: bytes=-{limit}`, which a store such as S3 answers with
    /// them; a log GitHub answers with itself, unredirected, is asked for
    /// with no range. GitHub's own store ignores that range and answers with
    /// the whole log. When it says how long the log is and that it serves
    /// ranges of bytes, that answer is left unread and the log is asked for
    /// again, without the token and in what is left of the same time, from
    /// `limit` bytes before its end, with `Range: bytes={start}-`. A log
    /// whose store says neither is read to its end, keeping only its last
    /// `limit` bytes and never holding more than twice `limit`.
    ///
    /// A part of the log a store answers with, as 206, is its end when its
    /// `Content-Range` spans exactly the bytes that arrived and ends where
    /// the log does, and the log began before it unless it starts at the
    /// first byte; a part that is not is taken to follow more of the log. A
    /// store that refuses the range as 416 with a `Content-Range` of
    /// `bytes */0` holds an empty log, and any other 416 is
    /// [`JobLogError::Refused`]. A `limit` of zero asks for no range and
    /// says only whether the log held anything, from the length the answer
    /// gives where it gives one.
    ///
    /// The bytes start at a character: those of a UTF-8 character the cut
    /// fell inside are left out, so a log that began before them comes back
    /// up to three bytes short of `limit`.
    pub async fn fetch_job_log_tail(
        &self,
        repository: &Repository,
        token: &SecretValue,
        job: NonZeroU64,
        limit: usize,
    ) -> PullRequestResult<JobLog> {
        self.job_log(repository, token, job, limit, LogEnd::End)
            .await
    }

    /// The `end` of the log of `job` in `repository`, up to `limit` bytes,
    /// read as [`Self::fetch_job_log`] says.
    async fn job_log(
        &self,
        repository: &Repository,
        token: &SecretValue,
        job: NonZeroU64,
        limit: usize,
        end: LogEnd,
    ) -> PullRequestResult<JobLog> {
        let job = job.to_string();
        let url = self.repository_endpoint(repository, &["actions", "jobs", &job, "logs"])?;
        let (response, sent_at) = self
            .attempted(self.request_on(&self.writer, Method::GET, url, token, ACCEPT)?)
            .await?;
        let deadline = sent_at.checked_add(self.timeout);

        let status = response.status();
        if status.is_success() {
            return logged(response, limit, end).await;
        }
        if !status.is_redirection() {
            return Err(refusal(response).await);
        }

        let target = destination(&self.origin, &response)?;
        self.stored(target, deadline, limit, end).await
    }

    /// The `end` of the log stored at `target`, up to `limit` bytes, asked
    /// for as [`Self::download`] says. A store that ignores the range the
    /// end is asked for by, but gives the log's length and serves ranges of
    /// bytes, is asked again from `limit` bytes before its end.
    async fn stored(
        &self,
        target: Url,
        deadline: Option<Instant>,
        limit: usize,
        end: LogEnd,
    ) -> PullRequestResult<JobLog> {
        let range = end.range(limit);
        let stored = self
            .download(target.clone(), deadline, range.as_deref())
            .await?;
        let status = stored.status();
        if range.is_some()
            && status == StatusCode::RANGE_NOT_SATISFIABLE
            && ContentRange::unsatisfied(stored.headers()) == Some(0)
        {
            return Ok(JobLog::new(Vec::new(), false));
        }
        let stored = served(stored)?;
        let resumed = match range.and_then(|_| resumable(&stored, limit)) {
            Some(start) => {
                drop(stored);
                let range = format!("{BYTES_UNIT}={start}-");
                served(self.download(target, deadline, Some(&range)).await?)?
            }
            None => stored,
        };
        logged(resumed, limit, end).await
    }

    /// The answer of the store at `target` to a request for the log, which
    /// carries `range` if any, with no token, in what is left of the time
    /// before `deadline`; none left is [`JobLogError::Unreachable`], unsent.
    /// A target on this service's origin is asked under the API version this
    /// service names, if any, and one anywhere else under none.
    async fn download(
        &self,
        target: Url,
        deadline: Option<Instant>,
        range: Option<&str>,
    ) -> Result<Response, JobLogError> {
        let left = match deadline {
            Some(deadline) => remaining(deadline).ok_or(JobLogError::Unreachable)?,
            None => self.timeout,
        };
        let versioned = self
            .api_version
            .as_deref()
            .filter(|_| self.origin.holds(&target));
        let request = self.writer.get(target).timeout(left);
        let request = match versioned {
            Some(version) => request.header(API_VERSION, version),
            None => request,
        };
        let request = match range {
            Some(range) => request.header(header::RANGE, range),
            None => request,
        };
        request.send().await.map_err(|_| JobLogError::Unreachable)
    }
}

/// `stored`, when it is a success; any other status is
/// [`JobLogError::Refused`].
fn served(stored: Response) -> Result<Response, JobLogError> {
    match stored.status().is_success() {
        true => Ok(stored),
        false => Err(JobLogError::Refused(stored.status().as_u16())),
    }
}

/// Where the last `limit` bytes of the whole log `stored` answered with
/// start, when it says the log is longer than that and that it serves ranges
/// of bytes.
fn resumable(stored: &Response, limit: usize) -> Option<u64> {
    let whole = stored.status() == StatusCode::OK;
    let length = stored.content_length()?;
    let ranged = stored
        .headers()
        .get(header::ACCEPT_RANGES)
        .and_then(|accepted| accepted.to_str().ok())
        .is_some_and(|accepted| {
            accepted
                .split(',')
                .any(|unit| unit.trim().eq_ignore_ascii_case(BYTES_UNIT))
        });
    (whole && ranged)
        .then(|| length.checked_sub(counted(limit)))
        .flatten()
        .filter(|start| *start > 0)
}

/// Whether a walk that read `read` rows reached every one GitHub `counted`:
/// exactly that many, or more on a walk that `ended` on a short page. Without
/// a count, only a walk that ended on a short page did.
fn read_as_counted(read: u64, counted: Option<u64>, ended: bool) -> bool {
    match counted {
        Some(counted) => read == counted || (ended && read > counted),
        None => ended,
    }
}

/// A workflow run GitHub listed, its commit parsed.
fn run(row: GitHubWorkflowRun) -> PullRequestResult<WorkflowRun> {
    Ok(WorkflowRun {
        id: row.id,
        workflow_id: row.workflow_id,
        attempt: row.run_attempt,
        name: row.name,
        path: row.path,
        event: row.event,
        status: row.status,
        conclusion: row.conclusion,
        head_branch: row.head_branch,
        head_sha: CommitSha::parse(&row.head_sha)?,
        created_at: row.created_at,
        started_at: row.run_started_at,
        updated_at: row.updated_at,
        url: row.html_url,
        repository: row.repository.full_name,
    })
}

/// A workflow job GitHub answered with, its commit parsed.
fn workflow_job(job: GitHubWorkflowJob) -> PullRequestResult<WorkflowJob> {
    Ok(WorkflowJob {
        id: job.id,
        run_id: job.run_id,
        name: job.name,
        status: job.status,
        conclusion: job.conclusion,
        head_sha: CommitSha::parse(&job.head_sha)?,
        url: job.html_url,
        started_at: job.started_at,
        completed_at: job.completed_at,
    })
}

/// The `end` of a log `response` carries, up to `limit` bytes, and whether
/// the log held more; a body that stops arriving is
/// [`JobLogError::Interrupted`].
async fn logged(response: Response, limit: usize, end: LogEnd) -> PullRequestResult<JobLog> {
    let read = match end {
        LogEnd::Start => read_prefix(response, limit).await,
        LogEnd::End => read_tail(response, limit).await,
    };
    let (content, truncated) = read.map_err(|_| JobLogError::Interrupted)?;
    Ok(JobLog::new(content, truncated))
}

/// The last `limit` bytes of a log `response` carries, from the first
/// character in them when the log held more, and whether it did: more
/// arrived than `limit`, or a partial answer's `Content-Range` does not
/// place what arrived as the whole log.
async fn read_tail(mut response: Response, limit: usize) -> PullRequestResult<(Vec<u8>, bool)> {
    if limit == 0 {
        return Ok((Vec::new(), held_any(response).await?));
    }
    let partial = response.status() == StatusCode::PARTIAL_CONTENT;
    let range = ContentRange::of(response.headers());
    let mut tail = Vec::new();
    let mut arrived: u64 = 0;
    while let Some(chunk) = response.chunk().await? {
        arrived = arrived.saturating_add(counted(chunk.len()));
        keep_last(&mut tail, &chunk, limit);
    }
    tail.drain(..tail.len().saturating_sub(limit));
    let overflowed = arrived > counted(limit);
    let followed = partial
        && range
            .and_then(|range| range.began_before(arrived))
            .unwrap_or(true);
    let truncated = overflowed || followed;
    if truncated {
        from_character(&mut tail);
    }
    Ok((tail, truncated))
}

/// Whether the log `response` carries holds any bytes, as its length says
/// where it gives one, and otherwise as far as its first that arrive.
async fn held_any(mut response: Response) -> PullRequestResult<bool> {
    if let Some(length) = response.content_length() {
        return Ok(length > 0);
    }
    while let Some(chunk) = response.chunk().await? {
        if !chunk.is_empty() {
            return Ok(true);
        }
    }
    Ok(false)
}

/// `length`, a count of bytes in memory, as a count of bytes on the wire.
fn counted(length: usize) -> u64 {
    u64::try_from(length).unwrap_or(u64::MAX)
}

/// `chunk` added to the end of `tail`, whose last `limit` bytes are then
/// the last that arrived, and which neither holds nor has room for more
/// than twice `limit`.
fn keep_last(tail: &mut Vec<u8>, chunk: &[u8], limit: usize) {
    if chunk.len() >= limit {
        tail.clear();
        tail.reserve_exact(limit);
        tail.extend_from_slice(&chunk[chunk.len() - limit..]);
        return;
    }
    let bound = limit.saturating_mul(2);
    if tail.len() + chunk.len() > bound {
        tail.drain(..tail.len() - limit);
    }
    let wanted = (tail.len() + chunk.len())
        .max(tail.capacity().saturating_mul(2))
        .min(bound);
    tail.reserve_exact(wanted - tail.len());
    tail.extend_from_slice(chunk);
}

/// `tail` from its first UTF-8 character, without the up to three
/// continuation bytes of one the cut fell inside. Bytes that run on as
/// continuations past any character's are left as they are, being no UTF-8
/// to cut a character of.
fn from_character(tail: &mut Vec<u8>) {
    let continuing = tail
        .iter()
        .take(LONGEST_CONTINUATION + 1)
        .take_while(|byte| *byte & CONTINUATION_MASK == CONTINUATION)
        .count();
    if continuing <= LONGEST_CONTINUATION {
        tail.drain(..continuing);
    }
}

/// Where a redirect of the request for a log sends it on to, if anywhere it
/// may go.
fn destination(origin: &Origin, response: &Response) -> Result<Url, JobLogError> {
    let location = response
        .headers()
        .get(header::LOCATION)
        .and_then(|location| location.to_str().ok())
        .ok_or(JobLogError::Undirected)?;
    followed(origin, response.url(), location)
}

/// `location`, resolved against `from`, when it is on `origin` or is an
/// HTTPS address, on its default port, on a host GitHub stores logs on, and
/// carries no credentials.
fn followed(origin: &Origin, from: &Url, location: &str) -> Result<Url, JobLogError> {
    let target = from
        .join(location)
        .map_err(|_| JobLogError::UnexpectedHost)?;
    let credentialed = !target.username().is_empty() || target.password().is_some();
    match !credentialed && (origin.holds(&target) || stores_logs(&target)) {
        true => Ok(target),
        false => Err(JobLogError::UnexpectedHost),
    }
}

/// Whether `target` is an HTTPS address, on HTTPS's default port, on one of
/// [`LOG_HOSTS`] or a host under one.
fn stores_logs(target: &Url) -> bool {
    target.scheme() == SECURE_SCHEME
        && target.port().is_none()
        && target.host_str().is_some_and(|host| {
            LOG_HOSTS.iter().any(|stored| {
                host.strip_suffix(stored)
                    .is_some_and(|under| under.is_empty() || under.ends_with('.'))
            })
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::branch_name::BranchName;
    use crate::pull_request::service::fixtures::commit;
    use crate::pull_request::service::fixtures::enterprise;
    use crate::pull_request::service::fixtures::hanging_up;
    use crate::pull_request::service::fixtures::project;
    use crate::pull_request::service::fixtures::query_is;
    use crate::pull_request::service::fixtures::stand_in;
    use crate::pull_request::service::fixtures::token;
    use serde_json::Value;
    use serde_json::json;
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::header;
    use wiremock::matchers::header_exists;
    use wiremock::matchers::method;
    use wiremock::matchers::path;
    use wiremock::matchers::query_param;

    const JOB_LOG: &str = "/repos/acme/project/actions/jobs/9/logs";

    const STORED_LOG: &str = "/storage/log";

    const SIGNED_LOG: &str = "/storage/log?sig=signed-secret";

    const RUNS: &str = "/repos/acme/project/actions/runs";

    const JOB: &str = "/repos/acme/project/actions/jobs/9";

    /// More bytes than a limit in these tests reads.
    const LONG_LOG_BYTES: usize = 5_000;

    /// The limit a long log is read up to.
    const LIMIT: usize = 1_000;

    fn job() -> NonZeroU64 {
        NonZeroU64::new(9).unwrap()
    }

    fn redirect(location: &str) -> ResponseTemplate {
        ResponseTemplate::new(302).insert_header("location", location)
    }

    async fn log(service: &PullRequestService, limit: usize) -> PullRequestResult<JobLog> {
        service
            .fetch_job_log(&project(service), &token(), job(), limit)
            .await
    }

    async fn redirecting(server: &MockServer, location: &str) {
        Mock::given(method("GET"))
            .and(path(JOB_LOG))
            .respond_with(redirect(location))
            .mount(server)
            .await;
    }

    async fn storing(server: &MockServer, answer: ResponseTemplate) {
        Mock::given(method("GET"))
            .and(path(STORED_LOG))
            .respond_with(answer)
            .mount(server)
            .await;
    }

    /// A run numbered `id` of one workflow at `sha`, with every field GitHub
    /// always sends and none it may leave out.
    fn run_row(id: usize, sha: &CommitSha) -> Value {
        json!({
            "id": id,
            "workflow_id": 1,
            "path": ".github/workflows/ci.yml",
            "event": "push",
            "head_sha": sha.as_str(),
            "created_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-01T00:09:00Z",
            "html_url": format!("https://github.com/acme/project/actions/runs/{id}"),
            "repository": { "full_name": "acme/project" },
        })
    }

    /// `count` runs of one workflow at one commit, numbered on from `first`.
    fn runs(first: usize, count: usize, sha: &CommitSha) -> Vec<Value> {
        (first..first + count).map(|id| run_row(id, sha)).collect()
    }

    /// A job with every field GitHub always sends and none it may leave out.
    fn job_row(sha: &CommitSha) -> Value {
        json!({
            "id": 9,
            "run_id": 30,
            "name": "test",
            "status": "completed",
            "head_sha": sha.as_str(),
        })
    }

    async fn collect(
        service: &PullRequestService,
        filter: &WorkflowRunFilter,
    ) -> PullRequestResult<Collected<WorkflowRun>> {
        service
            .collect_workflow_runs(&project(service), &token(), filter)
            .await
    }

    /// What listing a page of one well-formed run and `row` comes to.
    async fn listing(row: Value) -> PullRequestResult<Collected<WorkflowRun>> {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(RUNS))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "total_count": 2,
                "workflow_runs": [run_row(1, &commit('a')), row],
            })))
            .expect(1)
            .mount(&server)
            .await;
        collect(&stand_in(&server).await, &WorkflowRunFilter::new()).await
    }

    /// What reading a job GitHub answers with `body` comes to.
    async fn reading(body: Value) -> PullRequestResult<WorkflowJob> {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(JOB))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .expect(1)
            .mount(&server)
            .await;
        let service = stand_in(&server).await;
        service
            .fetch_workflow_job(&project(&service), &token(), job())
            .await
    }

    /// Whether `read` is the error an answer this crate cannot read is.
    fn unreadable<T: std::fmt::Debug>(read: &PullRequestResult<T>) -> bool {
        matches!(read, Err(PullRequestError::GitHubApi(said)) if said == UNREADABLE)
    }

    /// `row` without `field`.
    fn without(mut row: Value, field: &str) -> Value {
        row.as_object_mut()
            .expect("a row is an object")
            .remove(field);
        row
    }

    /// `row` with `field` set to `value`.
    fn with(mut row: Value, field: &str, value: Value) -> Value {
        row[field] = value;
        row
    }

    /// A log that says it is longer than it is, then stops.
    const CUT_SHORT: &str =
        "HTTP/1.1 200 OK\r\ncontent-length: 1000\r\nconnection: close\r\n\r\npartial";

    /// A redirect of the request for a log to where it is stored, on the
    /// same origin.
    const REDIRECTED: &str = "HTTP/1.1 302 Found\r\nlocation: /storage/log?sig=signed-secret\r\ncontent-length: 0\r\nconnection: close\r\n\r\n";

    #[tokio::test]
    async fn a_log_redirected_within_the_origin_is_read_without_the_token() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(JOB_LOG))
            .and(header("authorization", "Bearer token"))
            .respond_with(redirect(SIGNED_LOG))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(STORED_LOG))
            .and(header_exists("authorization"))
            .respond_with(ResponseTemplate::new(500))
            .with_priority(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(STORED_LOG))
            .and(query_param("sig", "signed-secret"))
            .respond_with(ResponseTemplate::new(200).set_body_string("step one\nstep two\n"))
            .expect(1)
            .mount(&server)
            .await;

        let read = log(&stand_in(&server).await, LIMIT).await.unwrap();

        assert_eq!(read, JobLog::new(b"step one\nstep two\n".to_vec(), false));
    }

    /// The API version the versioned tests name.
    const VERSION: &str = "2026-03-10";

    #[tokio::test]
    async fn a_log_redirected_within_the_origin_is_asked_for_under_the_api_version() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(JOB_LOG))
            .and(header(API_VERSION, VERSION))
            .respond_with(redirect(SIGNED_LOG))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(STORED_LOG))
            .and(header(API_VERSION, VERSION))
            .respond_with(ResponseTemplate::new(200).set_body_string("step one\n"))
            .expect(1)
            .mount(&server)
            .await;
        let service = stand_in(&server).await.with_api_version(VERSION);

        let read = log(&service, LIMIT).await.unwrap();

        assert_eq!(read, JobLog::new(b"step one\n".to_vec(), false));
    }

    #[tokio::test]
    async fn a_log_stored_off_the_origin_is_asked_for_under_no_api_version() {
        let origin = MockServer::start().await;
        let store = MockServer::start().await;
        Mock::given(header_exists(API_VERSION))
            .respond_with(ResponseTemplate::new(500))
            .with_priority(1)
            .expect(0)
            .mount(&store)
            .await;
        Mock::given(method("GET"))
            .and(path(STORED_LOG))
            .respond_with(ResponseTemplate::new(200).set_body_string("step one\n"))
            .expect(2)
            .mount(&store)
            .await;
        let service = stand_in(&origin).await.with_api_version(VERSION);
        let target = Url::parse(&format!("{}{SIGNED_LOG}", store.uri())).unwrap();

        for end in [LogEnd::Start, LogEnd::End] {
            let read = service
                .stored(target.clone(), None, LIMIT, end)
                .await
                .unwrap();

            assert_eq!(read, JobLog::new(b"step one\n".to_vec(), false), "{end:?}");
        }
    }

    #[tokio::test]
    async fn a_log_redirected_to_a_host_that_stores_no_logs_is_not_followed() {
        let server = MockServer::start().await;
        redirecting(&server, "https://evil.test/x").await;

        let refused = log(&stand_in(&server).await, LIMIT).await;

        assert!(
            matches!(
                refused,
                Err(PullRequestError::JobLog(JobLogError::UnexpectedHost))
            ),
            "{refused:?}"
        );
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn a_log_redirect_that_names_nowhere_is_undirected() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(JOB_LOG))
            .respond_with(ResponseTemplate::new(302))
            .mount(&server)
            .await;

        let refused = log(&stand_in(&server).await, LIMIT).await;

        assert!(
            matches!(
                refused,
                Err(PullRequestError::JobLog(JobLogError::Undirected))
            ),
            "{refused:?}"
        );
    }

    #[test]
    fn a_log_is_followed_only_to_the_origin_or_over_https_to_where_logs_are_stored() {
        let origin = Origin::configured("https://api.github.com", false).unwrap();
        let from =
            Url::parse("https://api.github.com/repos/acme/project/actions/jobs/9/logs").unwrap();

        for accepted in [
            "https://x.blob.core.windows.net/logs/9?sig=1",
            "https://objects.githubusercontent.com/logs/9",
            "https://blob.core.windows.net/logs/9",
            "https://api.github.com/storage/9",
            "https://objects.githubusercontent.com:443/logs/9",
            "/storage/9",
        ] {
            assert!(
                followed(&origin, &from, accepted).is_ok(),
                "{accepted} should be followed"
            );
        }
        for refused in [
            "https://github.com.evil.test/logs/9",
            "https://evilblob.core.windows.net/logs/9",
            "http://objects.githubusercontent.com/logs/9",
            "https://user:pw@objects.githubusercontent.com/logs/9",
            "https://user@api.github.com/storage/9",
            "http://api.github.com/storage/9",
            "https://evil.test/logs/9",
            "https://objects.githubusercontent.com:8443/logs/9",
            "https://x.blob.core.windows.net:444/logs/9",
        ] {
            assert_eq!(
                followed(&origin, &from, refused),
                Err(JobLogError::UnexpectedHost),
                "{refused} should be refused"
            );
        }

        let enterprise = Origin::configured("https://github.example.com/api/v3", false).unwrap();
        let from = Url::parse("https://github.example.com/api/v3/repos/acme/project").unwrap();
        assert!(followed(&enterprise, &from, "/storage/9").is_ok());
        for refused in [
            "https://github.example.com:8443/storage/9",
            "http://github.example.com/storage/9",
            "https://other.example.com/storage/9",
        ] {
            assert_eq!(
                followed(&enterprise, &from, refused),
                Err(JobLogError::UnexpectedHost),
                "{refused} is off the origin and stores no logs"
            );
        }
    }

    #[tokio::test]
    async fn a_log_is_read_up_to_its_limit_and_says_whether_it_went_on() {
        let long = "x".repeat(LONG_LOG_BYTES);

        let direct = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(JOB_LOG))
            .respond_with(ResponseTemplate::new(200).set_body_string(long.clone()))
            .mount(&direct)
            .await;
        let stored = MockServer::start().await;
        redirecting(&stored, SIGNED_LOG).await;
        storing(
            &stored,
            ResponseTemplate::new(200).set_body_string(long.clone()),
        )
        .await;

        for server in [&direct, &stored] {
            let service = stand_in(server).await;

            let cut = log(&service, LIMIT).await.unwrap();
            assert_eq!(cut.content.len(), LIMIT);
            assert!(cut.truncated, "a log longer than the limit went on");

            let whole = log(&service, LONG_LOG_BYTES).await.unwrap();
            assert_eq!(whole.content, long.as_bytes());
            assert!(!whole.truncated, "a log within the limit did not go on");
        }
    }

    #[tokio::test]
    async fn a_refused_download_names_its_status_and_not_its_address() {
        let server = MockServer::start().await;
        redirecting(&server, SIGNED_LOG).await;
        storing(&server, ResponseTemplate::new(403)).await;

        let refused = log(&stand_in(&server).await, LIMIT).await.unwrap_err();

        assert!(
            matches!(refused, PullRequestError::JobLog(JobLogError::Refused(403))),
            "{refused:?}"
        );
        let said = format!("{refused} {refused:?}");
        assert!(!said.contains("sig="), "{said}");
        assert!(!said.contains("signed-secret"), "{said}");
    }

    #[tokio::test]
    async fn a_refused_request_for_a_log_is_reported_as_any_refusal_is() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(JOB_LOG))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        let refused = log(&stand_in(&server).await, LIMIT).await;

        assert!(
            matches!(refused, Err(PullRequestError::NotFound)),
            "{refused:?}"
        );
    }

    #[tokio::test]
    async fn a_log_cut_short_by_github_is_interrupted() {
        let service = hanging_up(|_| CUT_SHORT).await;

        let refused = log(&service, LIMIT).await;

        assert!(
            matches!(
                refused,
                Err(PullRequestError::JobLog(JobLogError::Interrupted))
            ),
            "{refused:?}"
        );
    }

    #[tokio::test]
    async fn a_log_cut_short_where_it_is_stored_is_interrupted() {
        let service = hanging_up(|line| match line.contains(JOB_LOG) {
            true => REDIRECTED,
            false => CUT_SHORT,
        })
        .await;

        let refused = log(&service, LIMIT).await.unwrap_err();

        assert!(
            matches!(refused, PullRequestError::JobLog(JobLogError::Interrupted)),
            "{refused:?}"
        );
        assert!(!format!("{refused:?}").contains("sig="), "{refused:?}");
    }

    #[tokio::test]
    async fn a_store_that_hangs_up_unanswered_is_unreachable() {
        let service = hanging_up(|line| match line.contains(JOB_LOG) {
            true => REDIRECTED,
            false => "",
        })
        .await;

        let refused = log(&service, LIMIT).await.unwrap_err();

        assert!(
            matches!(refused, PullRequestError::JobLog(JobLogError::Unreachable)),
            "{refused:?}"
        );
        assert!(!format!("{refused:?}").contains("sig="), "{refused:?}");
    }

    /// The one timeout both hops of a log read share.
    const SHARED_TIMEOUT: Duration = Duration::from_secs(4);

    /// How long the first hop takes to redirect: well within the timeout.
    const FIRST_HOP_DELAY: Duration = Duration::from_secs(2);

    /// How long the store takes to answer: within a fresh timeout, past what
    /// the first hop left of the shared one.
    const STORE_DELAY: Duration = Duration::from_secs(3);

    #[tokio::test]
    async fn both_hops_of_a_log_spend_one_timeout() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(JOB_LOG))
            .respond_with(redirect(SIGNED_LOG).set_delay(FIRST_HOP_DELAY))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(STORED_LOG))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string("step one\n")
                    .set_delay(STORE_DELAY),
            )
            .expect(1)
            .mount(&server)
            .await;
        let service = PullRequestService::addressing(
            Origin::standing_in_for("github.com", &server.uri()).unwrap(),
            false,
            SHARED_TIMEOUT,
        )
        .unwrap();

        let refused = log(&service, LIMIT).await;

        assert!(
            matches!(
                refused,
                Err(PullRequestError::JobLog(JobLogError::Unreachable))
            ),
            "the store is given only what the first hop left: {refused:?}"
        );
    }

    /// The timeout a log read that waits on a rate limit is given: enough for
    /// two loopback hops, less than the wait.
    const RETRIED_TIMEOUT: Duration = Duration::from_secs(2);

    /// The longest wait on a rate limit a log read takes.
    const RETRIED_CEILING: Duration = Duration::from_secs(5);

    #[tokio::test]
    async fn a_log_sent_again_after_a_wait_still_has_its_time_to_reach_the_store() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(JOB_LOG))
            .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "3"))
            .up_to_n_times(1)
            .with_priority(1)
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(JOB_LOG))
            .respond_with(redirect(SIGNED_LOG))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(STORED_LOG))
            .respond_with(ResponseTemplate::new(200).set_body_string("step one\n"))
            .expect(1)
            .mount(&server)
            .await;
        let service = PullRequestService::addressing(
            Origin::standing_in_for("github.com", &server.uri()).unwrap(),
            false,
            RETRIED_TIMEOUT,
        )
        .unwrap()
        .with_retry_after(RETRIED_CEILING);

        let read = log(&service, LIMIT).await;

        assert!(
            matches!(&read, Ok(log) if log.content == b"step one\n"),
            "the store is given the time the answered attempt left: {read:?}"
        );
    }

    #[tokio::test]
    async fn a_log_store_that_limits_the_rate_is_refused_and_not_waited_on() {
        let server = MockServer::start().await;
        redirecting(&server, SIGNED_LOG).await;
        Mock::given(method("GET"))
            .and(path(STORED_LOG))
            .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "0"))
            .expect(1)
            .mount(&server)
            .await;
        let service = stand_in(&server).await.with_retry_after(RETRIED_CEILING);

        let refused = log(&service, LIMIT).await.unwrap_err();

        assert!(
            matches!(refused, PullRequestError::JobLog(JobLogError::Refused(429))),
            "{refused:?}"
        );
    }

    #[tokio::test]
    async fn a_log_whose_time_ran_out_before_the_store_is_never_asked_for() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(STORED_LOG))
            .respond_with(ResponseTemplate::new(200).set_body_string("step one\n"))
            .expect(0)
            .mount(&server)
            .await;
        let service = stand_in(&server).await;
        let spent = Instant::now();
        let target = Url::parse(&format!("{}{SIGNED_LOG}", server.uri())).unwrap();

        for end in [LogEnd::Start, LogEnd::End] {
            let refused = service
                .stored(target.clone(), Some(spent), LIMIT, end)
                .await;

            assert!(
                matches!(
                    refused,
                    Err(PullRequestError::JobLog(JobLogError::Unreachable))
                ),
                "{end:?}: {refused:?}"
            );
        }
    }

    #[tokio::test]
    async fn a_store_that_redirects_is_refused_and_not_followed() {
        let server = MockServer::start().await;
        redirecting(&server, SIGNED_LOG).await;
        storing(&server, redirect("/storage/elsewhere")).await;
        Mock::given(method("GET"))
            .and(path("/storage/elsewhere"))
            .respond_with(ResponseTemplate::new(200).set_body_string("step one\n"))
            .expect(0)
            .mount(&server)
            .await;

        let refused = log(&stand_in(&server).await, LIMIT).await;

        assert!(
            matches!(
                refused,
                Err(PullRequestError::JobLog(JobLogError::Refused(302)))
            ),
            "{refused:?}"
        );
    }

    #[tokio::test]
    async fn a_renamed_private_repository_s_log_is_refused_as_not_found() {
        let server = MockServer::start().await;
        let renamed = "/repos/acme/renamed/actions/jobs/9/logs";
        redirecting(&server, renamed).await;
        Mock::given(method("GET"))
            .and(path(renamed))
            .and(header_exists("authorization"))
            .respond_with(ResponseTemplate::new(200).set_body_string("step one\n"))
            .with_priority(1)
            .expect(0)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(renamed))
            .respond_with(ResponseTemplate::new(404))
            .expect(1)
            .mount(&server)
            .await;

        let refused = log(&stand_in(&server).await, LIMIT).await;

        assert!(
            matches!(
                refused,
                Err(PullRequestError::JobLog(JobLogError::Refused(404)))
            ),
            "the token is not carried on, even within the origin: {refused:?}"
        );
    }

    /// Where GitHub Enterprise serves job 9's log, under its `/api/v3`.
    const ENTERPRISE_JOB_LOG: &str = "/api/v3/repos/acme/project/actions/jobs/9/logs";

    /// More bytes than a reader takes in one chunk, so a log this long
    /// arrives in many.
    const LARGE_LOG_BYTES: usize = 1024 * 1024;

    async fn tail(service: &PullRequestService, limit: usize) -> PullRequestResult<JobLog> {
        service
            .fetch_job_log_tail(&project(service), &token(), job(), limit)
            .await
    }

    /// A log of numbered lines, `length` bytes long, so that each stretch
    /// of it says where in the log it was cut from.
    fn numbered(length: usize) -> Vec<u8> {
        (0_u32..)
            .flat_map(|line| format!("line {line}\n").into_bytes())
            .take(length)
            .collect()
    }

    /// The last `limit` bytes of `log`.
    fn last(log: &[u8], limit: usize) -> Vec<u8> {
        log[log.len() - limit..].to_vec()
    }

    /// The `Range` a tail of `limit` bytes is asked for by.
    fn ranged(limit: usize) -> String {
        format!("bytes=-{limit}")
    }

    /// The `Range` the log is asked for again by, from `start` to its end.
    fn resumed(start: usize) -> String {
        format!("bytes={start}-")
    }

    /// A store's answer with the last `limit` bytes of `log` alone.
    fn partial(log: &[u8], limit: usize) -> ResponseTemplate {
        let first = log.len() - limit;
        ResponseTemplate::new(206)
            .insert_header(
                "content-range",
                format!("bytes {first}-{}/{}", log.len() - 1, log.len()).as_str(),
            )
            .set_body_bytes(last(log, limit))
    }

    /// A store's answer with the whole of `log`, which says that it serves
    /// ranges of bytes, as GitHub's own store answers a suffix range.
    fn whole_offering_ranges(log: &[u8]) -> ResponseTemplate {
        ResponseTemplate::new(200)
            .insert_header("accept-ranges", "bytes")
            .set_body_bytes(log.to_vec())
    }

    /// A store on `server` that refuses any request carrying the token.
    async fn refusing_the_token(server: &MockServer) {
        Mock::given(method("GET"))
            .and(path(STORED_LOG))
            .and(header_exists("authorization"))
            .respond_with(ResponseTemplate::new(500))
            .with_priority(1)
            .expect(0)
            .mount(server)
            .await;
    }

    /// A store on `server` that answers a request for the last `limit`
    /// bytes, without the token, with `answer`, as one that honours a
    /// suffix range does.
    async fn storing_tail(server: &MockServer, limit: usize, answer: ResponseTemplate) {
        refusing_the_token(server).await;
        Mock::given(method("GET"))
            .and(path(STORED_LOG))
            .and(query_param("sig", "signed-secret"))
            .and(header("range", ranged(limit).as_str()))
            .respond_with(answer)
            .expect(1)
            .mount(server)
            .await;
    }

    /// A store on `server` that keeps `log` as GitHub's own store does: it
    /// answers a request for the last `limit` bytes with the whole log, and
    /// one from `limit` bytes before its end with those bytes alone.
    async fn storing_as_github_does(server: &MockServer, log: &[u8], limit: usize) {
        storing_tail(server, limit, whole_offering_ranges(log)).await;
        Mock::given(method("GET"))
            .and(path(STORED_LOG))
            .and(query_param("sig", "signed-secret"))
            .and(header("range", resumed(log.len() - limit).as_str()))
            .respond_with(partial(log, limit))
            .expect(1)
            .mount(server)
            .await;
    }

    /// The requests that reached the store on `server`, in order.
    async fn downloads(server: &MockServer) -> Vec<wiremock::Request> {
        server
            .received_requests()
            .await
            .unwrap()
            .into_iter()
            .filter(|request| request.url.path() == STORED_LOG)
            .collect()
    }

    /// A request for job 9's log at `route` that carries the token and no
    /// range, redirected to where it is stored.
    async fn redirecting_unranged(server: &MockServer, route: &str) {
        Mock::given(method("GET"))
            .and(path(route))
            .and(header_exists("range"))
            .respond_with(ResponseTemplate::new(500))
            .with_priority(1)
            .expect(0)
            .mount(server)
            .await;
        Mock::given(method("GET"))
            .and(path(route))
            .and(header("authorization", "Bearer token"))
            .respond_with(redirect(SIGNED_LOG))
            .expect(1)
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn a_tail_from_a_store_that_ignores_a_suffix_range_is_asked_for_again_from_its_start() {
        let server = MockServer::start().await;
        let log = numbered(LONG_LOG_BYTES);
        redirecting_unranged(&server, JOB_LOG).await;
        storing_as_github_does(&server, &log, LIMIT).await;

        let read = tail(&stand_in(&server).await, LIMIT).await.unwrap();

        assert_eq!(read, JobLog::new(last(&log, LIMIT), true));
        let downloads = downloads(&server).await;
        let ranges: Vec<&str> = downloads
            .iter()
            .map(|request| request.headers["range"].to_str().unwrap())
            .collect();
        assert_eq!(
            ranges,
            [ranged(LIMIT), resumed(LONG_LOG_BYTES - LIMIT)],
            "the suffix first, then from where the last bytes start"
        );
        assert!(
            downloads
                .iter()
                .all(|request| !request.headers.contains_key("authorization")),
            "neither download carries the token"
        );
    }

    #[tokio::test]
    async fn a_tail_from_a_store_that_honours_a_suffix_range_is_read_as_it_answers() {
        let server = MockServer::start().await;
        let log = numbered(LONG_LOG_BYTES);
        redirecting_unranged(&server, JOB_LOG).await;
        storing_tail(&server, LIMIT, partial(&log, LIMIT)).await;

        let read = tail(&stand_in(&server).await, LIMIT).await.unwrap();

        assert_eq!(read, JobLog::new(last(&log, LIMIT), true));
        assert_eq!(downloads(&server).await.len(), 1);
    }

    #[tokio::test]
    async fn a_range_that_holds_the_whole_log_is_not_truncated() {
        let log = numbered(LIMIT / 2);

        for unit in ["bytes", "BYTES"] {
            let server = MockServer::start().await;
            let answer = ResponseTemplate::new(206)
                .insert_header(
                    "content-range",
                    format!("{unit} 0-{}/{}", log.len() - 1, log.len()).as_str(),
                )
                .set_body_bytes(log.clone());
            redirecting_unranged(&server, JOB_LOG).await;
            storing_tail(&server, LIMIT, answer).await;

            let read = tail(&stand_in(&server).await, LIMIT).await.unwrap();

            assert_eq!(read, JobLog::new(log.clone(), false), "{unit}");
        }
    }

    #[tokio::test]
    async fn a_range_that_does_not_end_the_log_is_taken_as_truncated() {
        for content_range in ["bytes 0-8/100", "bytes 3-11/100", "bytes 0-3/9"] {
            let server = MockServer::start().await;
            let answer = ResponseTemplate::new(206)
                .insert_header("content-range", content_range)
                .set_body_string("step one\n");
            redirecting_unranged(&server, JOB_LOG).await;
            storing_tail(&server, LIMIT, answer).await;

            let read = tail(&stand_in(&server).await, LIMIT).await.unwrap();

            assert_eq!(
                read,
                JobLog::new(b"step one\n".to_vec(), true),
                "{content_range}"
            );
        }
    }

    #[tokio::test]
    async fn a_range_whose_extent_cannot_be_read_is_taken_as_truncated() {
        for content_range in [None, Some("bytes */9"), Some("items 0-8/9")] {
            let server = MockServer::start().await;
            let answer = ResponseTemplate::new(206).set_body_string("step one\n");
            let answer = match content_range {
                Some(content_range) => answer.insert_header("content-range", content_range),
                None => answer,
            };
            redirecting_unranged(&server, JOB_LOG).await;
            storing_tail(&server, LIMIT, answer).await;

            let read = tail(&stand_in(&server).await, LIMIT).await.unwrap();

            assert_eq!(
                read,
                JobLog::new(b"step one\n".to_vec(), true),
                "{content_range:?}"
            );
        }
    }

    #[tokio::test]
    async fn a_tail_from_a_store_that_offers_no_ranges_is_read_to_its_end_keeping_the_last_bytes() {
        let log = numbered(LARGE_LOG_BYTES);

        let direct = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(JOB_LOG))
            .and(header_exists("range"))
            .respond_with(ResponseTemplate::new(500))
            .with_priority(1)
            .expect(0)
            .mount(&direct)
            .await;
        Mock::given(method("GET"))
            .and(path(JOB_LOG))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(log.clone()))
            .expect(1)
            .mount(&direct)
            .await;
        let stored = MockServer::start().await;
        redirecting_unranged(&stored, JOB_LOG).await;
        storing_tail(
            &stored,
            LIMIT,
            ResponseTemplate::new(200).set_body_bytes(log.clone()),
        )
        .await;

        for server in [&direct, &stored] {
            let read = tail(&stand_in(server).await, LIMIT).await.unwrap();

            assert_eq!(read, JobLog::new(last(&log, LIMIT), true));
        }
    }

    #[tokio::test]
    async fn a_log_within_the_limit_of_its_tail_is_read_whole() {
        let log = numbered(LIMIT);

        for whole in [b"step one\n".to_vec(), log] {
            let server = MockServer::start().await;
            redirecting_unranged(&server, JOB_LOG).await;
            storing_tail(&server, LIMIT, whole_offering_ranges(&whole)).await;

            let read = tail(&stand_in(&server).await, LIMIT).await.unwrap();

            assert_eq!(read, JobLog::new(whole, false));
            assert_eq!(
                downloads(&server).await.len(),
                1,
                "a log within the limit is not asked for again"
            );
        }
    }

    #[tokio::test]
    async fn a_tail_cut_inside_a_character_starts_at_the_next_one() {
        let log = "é".repeat(LONG_LOG_BYTES / 2).into_bytes();
        let odd = LIMIT + 1;
        let expected = JobLog::new(last(&log, LIMIT), true);

        let ranged = MockServer::start().await;
        redirecting_unranged(&ranged, JOB_LOG).await;
        storing_tail(&ranged, odd, partial(&log, odd)).await;
        let asked_again = MockServer::start().await;
        redirecting_unranged(&asked_again, JOB_LOG).await;
        storing_as_github_does(&asked_again, &log, odd).await;
        let ignored = MockServer::start().await;
        redirecting_unranged(&ignored, JOB_LOG).await;
        storing_tail(
            &ignored,
            odd,
            ResponseTemplate::new(200).set_body_bytes(log.clone()),
        )
        .await;

        for server in [&ranged, &asked_again, &ignored] {
            let read = tail(&stand_in(server).await, odd).await.unwrap();

            assert_eq!(read, expected);
            assert!(std::str::from_utf8(&read.content).is_ok());
        }
    }

    #[tokio::test]
    async fn an_enterprise_tail_is_read_through_its_api_v3_origin() {
        let server = MockServer::start().await;
        let log = numbered(LONG_LOG_BYTES);
        redirecting_unranged(&server, ENTERPRISE_JOB_LOG).await;
        storing_tail(&server, LIMIT, partial(&log, LIMIT)).await;
        let service = enterprise(&server).await;

        let read = service
            .fetch_job_log_tail(&project(&service), &token(), job(), LIMIT)
            .await
            .unwrap();

        assert_eq!(read, JobLog::new(last(&log, LIMIT), true));
    }

    /// What a store that refuses the range of a tail of `limit` bytes as
    /// 416, its `Content-Range` saying `content_range`, comes to.
    async fn unsatisfied(limit: usize, content_range: &str) -> PullRequestResult<JobLog> {
        let server = MockServer::start().await;
        redirecting_unranged(&server, JOB_LOG).await;
        Mock::given(method("GET"))
            .and(path(STORED_LOG))
            .respond_with(ResponseTemplate::new(416).insert_header("content-range", content_range))
            .expect(1)
            .mount(&server)
            .await;
        tail(&stand_in(&server).await, limit).await
    }

    #[tokio::test]
    async fn a_store_that_finds_the_log_empty_holds_an_empty_log() {
        let read = unsatisfied(LIMIT, "bytes */0").await.unwrap();

        assert_eq!(read, JobLog::new(Vec::new(), false));
    }

    #[tokio::test]
    async fn a_store_that_refuses_the_range_of_a_log_with_bytes_is_refused() {
        let refused = unsatisfied(LIMIT, "bytes */15741").await;

        assert!(
            matches!(
                refused,
                Err(PullRequestError::JobLog(JobLogError::Refused(416)))
            ),
            "{refused:?}"
        );
    }

    #[tokio::test]
    async fn a_store_that_refuses_a_range_never_asked_for_is_refused() {
        let refused = unsatisfied(0, "bytes */0").await;

        assert!(
            matches!(
                refused,
                Err(PullRequestError::JobLog(JobLogError::Refused(416)))
            ),
            "{refused:?}"
        );
    }

    #[tokio::test]
    async fn a_tail_of_no_bytes_asks_for_no_range_and_says_whether_the_log_held_any() {
        for (body, held) in [("step one\n", true), ("", false)] {
            let server = MockServer::start().await;
            redirecting_unranged(&server, JOB_LOG).await;
            Mock::given(method("GET"))
                .and(path(STORED_LOG))
                .and(header_exists("range"))
                .respond_with(ResponseTemplate::new(416))
                .with_priority(1)
                .expect(0)
                .mount(&server)
                .await;
            storing(&server, ResponseTemplate::new(200).set_body_string(body)).await;

            let read = tail(&stand_in(&server).await, 0).await.unwrap();

            assert_eq!(read, JobLog::new(Vec::new(), held), "{body:?}");
        }
    }

    #[tokio::test]
    async fn a_tail_sent_again_after_a_wait_still_has_its_time_to_reach_the_store() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(JOB_LOG))
            .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "3"))
            .up_to_n_times(1)
            .with_priority(1)
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(JOB_LOG))
            .respond_with(redirect(SIGNED_LOG))
            .expect(1)
            .mount(&server)
            .await;
        storing_tail(
            &server,
            LIMIT,
            ResponseTemplate::new(200).set_body_string("step one\n"),
        )
        .await;
        let service = PullRequestService::addressing(
            Origin::standing_in_for("github.com", &server.uri()).unwrap(),
            false,
            RETRIED_TIMEOUT,
        )
        .unwrap()
        .with_retry_after(RETRIED_CEILING);

        let read = tail(&service, LIMIT).await;

        assert!(
            matches!(&read, Ok(log) if log.content == b"step one\n" && !log.truncated),
            "the store is given the time the answered attempt left: {read:?}"
        );
    }

    #[tokio::test]
    async fn a_tail_store_that_limits_the_rate_is_refused_and_not_waited_on() {
        let server = MockServer::start().await;
        redirecting_unranged(&server, JOB_LOG).await;
        storing_tail(
            &server,
            LIMIT,
            ResponseTemplate::new(429).insert_header("retry-after", "0"),
        )
        .await;
        let service = stand_in(&server).await.with_retry_after(RETRIED_CEILING);

        let refused = tail(&service, LIMIT).await.unwrap_err();

        assert!(
            matches!(refused, PullRequestError::JobLog(JobLogError::Refused(429))),
            "{refused:?}"
        );
    }

    #[tokio::test]
    async fn a_refused_request_for_a_tail_is_reported_as_any_refusal_is() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(JOB_LOG))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;

        let refused = tail(&stand_in(&server).await, LIMIT).await;

        assert!(
            matches!(refused, Err(PullRequestError::NotFound)),
            "{refused:?}"
        );
    }

    #[tokio::test]
    async fn a_tail_redirected_to_a_host_that_stores_no_logs_is_not_followed() {
        let server = MockServer::start().await;
        redirecting(&server, "https://evil.test/x").await;

        let refused = tail(&stand_in(&server).await, LIMIT).await;

        assert!(
            matches!(
                refused,
                Err(PullRequestError::JobLog(JobLogError::UnexpectedHost))
            ),
            "{refused:?}"
        );
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn a_tail_cut_short_is_interrupted() {
        let direct = hanging_up(|_| CUT_SHORT).await;
        let stored = hanging_up(|line| match line.contains(JOB_LOG) {
            true => REDIRECTED,
            false => CUT_SHORT,
        })
        .await;

        for service in [&direct, &stored] {
            let refused = tail(service, LIMIT).await.unwrap_err();

            assert!(
                matches!(refused, PullRequestError::JobLog(JobLogError::Interrupted)),
                "{refused:?}"
            );
            assert!(!format!("{refused:?}").contains("sig="), "{refused:?}");
        }
    }

    #[test]
    fn a_rolling_tail_keeps_the_last_bytes_and_never_more_than_twice_its_limit() {
        let log = numbered(LONG_LOG_BYTES);

        for limit in [1, LIMIT] {
            for size in [1, 2, 7, LIMIT - 1, LIMIT, LIMIT + 1, LONG_LOG_BYTES] {
                let mut kept = Vec::new();
                for chunk in log.chunks(size) {
                    keep_last(&mut kept, chunk, limit);
                    assert!(
                        kept.capacity() <= 2 * limit,
                        "chunks of {size} grew a tail of {limit} to {}",
                        kept.capacity()
                    );
                }
                kept.drain(..kept.len().saturating_sub(limit));

                assert_eq!(kept, last(&log, limit), "chunks of {size}, limit {limit}");
            }
        }
    }

    #[test]
    fn a_tail_starts_at_the_first_character_after_its_cut() {
        let text = "a😀b".as_bytes();

        for (cut, expected) in [
            (0, "a😀b"),
            (1, "😀b"),
            (2, "b"),
            (3, "b"),
            (4, "b"),
            (5, "b"),
        ] {
            let mut tail = text[cut..].to_vec();
            from_character(&mut tail);

            assert_eq!(tail, expected.as_bytes(), "cut at {cut}");
        }

        for unencoded in [vec![0x80; 4], vec![0x80, 0x80, 0x80, 0x80, b'a']] {
            let mut tail = unencoded.clone();
            from_character(&mut tail);

            assert_eq!(tail, unencoded, "no UTF-8 is left as it is");
        }
    }

    #[tokio::test]
    async fn the_run_filter_reaches_the_query_and_pages_are_followed_until_a_short_one() {
        let server = MockServer::start().await;
        let sha = commit('a');
        for (page, first, count) in [("1", 1, PAGE_SIZE), ("2", PAGE_SIZE + 1, 3)] {
            Mock::given(method("GET"))
                .and(path(RUNS))
                .and(query_is(&[
                    ("head_sha", sha.as_str()),
                    ("event", "release"),
                    ("branch", "main"),
                    ("created", ">=2026-01-01"),
                    ("per_page", "100"),
                    ("page", page),
                ]))
                .respond_with(ResponseTemplate::new(200).set_body_json(
                    json!({ "total_count": PAGE_SIZE + 3, "workflow_runs": runs(first, count, &sha) }),
                ))
                .expect(1)
                .mount(&server)
                .await;
        }
        let service = stand_in(&server).await;
        let filter = WorkflowRunFilter::new()
            .with_head_sha(sha.clone())
            .with_event("release")
            .with_branch(BranchName::parse("main").unwrap())
            .with_created(">=2026-01-01");

        let listed = collect(&service, &filter).await.unwrap();

        assert_eq!(listed.items.len(), PAGE_SIZE + 3);
        assert!(listed.items.iter().all(|run| run.head_sha == sha));
        assert!(listed.complete, "every counted run was read");
    }

    #[tokio::test]
    async fn an_unfiltered_listing_asks_for_nothing_but_its_page() {
        let server = MockServer::start().await;
        let sha = commit('a');
        Mock::given(method("GET"))
            .and(path(RUNS))
            .and(query_is(&[("per_page", "100"), ("page", "1")]))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({ "total_count": 1, "workflow_runs": runs(1, 1, &sha) })),
            )
            .expect(1)
            .mount(&server)
            .await;

        let listed = collect(&stand_in(&server).await, &WorkflowRunFilter::new())
            .await
            .unwrap();

        assert_eq!(
            listed,
            Collected::new(
                vec![WorkflowRun::new(
                    NonZeroU64::new(1).unwrap(),
                    NonZeroU64::new(1).unwrap(),
                    ".github/workflows/ci.yml",
                    "push",
                    sha,
                    "2026-01-01T00:00:00Z",
                    "2026-01-01T00:09:00Z",
                    "https://github.com/acme/project/actions/runs/1",
                    "acme/project",
                )],
                true,
            )
        );
    }

    /// What walking ten full pages of runs, each saying GitHub counted
    /// `counted`, comes to, filtered by `filter`.
    async fn capped(counted: Option<usize>, filter: &WorkflowRunFilter) -> Collected<WorkflowRun> {
        let server = MockServer::start().await;
        let sha = commit('0');
        for number in 1..=MAXIMUM_PAGES {
            let first = (number - 1) * PAGE_SIZE + 1;
            let mut page = json!({ "workflow_runs": runs(first, PAGE_SIZE, &sha) });
            if let Some(counted) = counted {
                page["total_count"] = json!(counted);
            }
            Mock::given(method("GET"))
                .and(path(RUNS))
                .and(query_param("page", number.to_string()))
                .respond_with(ResponseTemplate::new(200).set_body_json(page))
                .expect(1)
                .mount(&server)
                .await;
        }
        collect(&stand_in(&server).await, filter).await.unwrap()
    }

    #[tokio::test]
    async fn exactly_as_many_runs_as_github_counted_are_complete_on_a_full_tenth_page() {
        let filter = WorkflowRunFilter::new().with_head_sha(commit('0'));

        let listed = capped(Some(PAGE_SIZE * MAXIMUM_PAGES), &filter).await;

        assert_eq!(listed.items.len(), PAGE_SIZE * MAXIMUM_PAGES);
        assert!(
            listed.complete,
            "1,000 runs read of 1,000 counted are all of them"
        );
    }

    #[tokio::test]
    async fn more_runs_than_github_lists_are_incomplete_and_never_an_error() {
        let filter = WorkflowRunFilter::new().with_head_sha(commit('0'));

        let listed = capped(Some(PAGE_SIZE * MAXIMUM_PAGES + 1), &filter).await;

        assert_eq!(listed.items.len(), PAGE_SIZE * MAXIMUM_PAGES);
        assert!(
            !listed.complete,
            "1,000 runs read of 1,001 counted are not all of them"
        );
    }

    #[tokio::test]
    async fn an_unfiltered_walk_past_the_cap_is_incomplete() {
        for counted in [Some(PAGE_SIZE * MAXIMUM_PAGES + PAGE_SIZE * 5), None] {
            let listed = capped(counted, &WorkflowRunFilter::new()).await;

            assert_eq!(listed.items.len(), PAGE_SIZE * MAXIMUM_PAGES);
            assert!(
                !listed.complete,
                "a full tenth page of an unfiltered listing counting {counted:?} is not every run"
            );
        }
    }

    #[tokio::test]
    async fn a_run_listed_on_two_pages_is_incomplete_though_as_many_were_read_as_counted() {
        let server = MockServer::start().await;
        let sha = commit('2');
        for (page, first, count) in [("1", 1, PAGE_SIZE), ("2", PAGE_SIZE, 50)] {
            Mock::given(method("GET"))
                .and(path(RUNS))
                .and(query_param("page", page))
                .respond_with(ResponseTemplate::new(200).set_body_json(
                    json!({ "total_count": PAGE_SIZE + 50, "workflow_runs": runs(first, count, &sha) }),
                ))
                .expect(1)
                .mount(&server)
                .await;
        }

        let listed = collect(&stand_in(&server).await, &WorkflowRunFilter::new())
            .await
            .unwrap();

        assert_eq!(listed.items.len(), PAGE_SIZE + 50);
        assert!(
            !listed.complete,
            "a run listed twice moved between pages, and another may have moved past them"
        );
    }

    #[tokio::test]
    async fn a_walk_that_ends_short_of_githubs_count_is_incomplete() {
        let server = MockServer::start().await;
        let sha = commit('1');
        Mock::given(method("GET"))
            .and(path(RUNS))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({ "total_count": 5, "workflow_runs": runs(1, 3, &sha) })),
            )
            .expect(1)
            .mount(&server)
            .await;

        let listed = collect(&stand_in(&server).await, &WorkflowRunFilter::new())
            .await
            .unwrap();

        assert_eq!(listed.items.len(), 3);
        assert!(
            !listed.complete,
            "3 runs read of 5 counted are not all of them"
        );
    }

    #[test]
    fn a_walk_reads_as_counted_only_when_it_read_every_counted_row() {
        for (read, counted, ended, expected) in [
            (1_000, Some(1_000), false, true),
            (1_000, Some(1_001), false, false),
            (1_000, Some(999), false, false),
            (1_000, None, false, false),
            (3, Some(3), true, true),
            (3, Some(2), true, true),
            (3, Some(5), true, false),
            (3, None, true, true),
        ] {
            assert_eq!(
                read_as_counted(read, counted, ended),
                expected,
                "{read} read of {counted:?} counted, ended on a short page: {ended}"
            );
        }
    }

    #[tokio::test]
    async fn a_run_keeps_githubs_words_and_tolerates_what_github_leaves_out() {
        let server = MockServer::start().await;
        let sha = commit('b');
        Mock::given(method("GET"))
            .and(path(RUNS))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "total_count": 2,
                "workflow_runs": [
                    {
                        "id": 30,
                        "workflow_id": 4,
                        "run_attempt": 2,
                        "name": "Release",
                        "path": ".github/workflows/release.yml",
                        "event": "release",
                        "status": "completed",
                        "conclusion": "success",
                        "head_branch": "main",
                        "head_sha": sha.as_str(),
                        "created_at": "2026-01-01T00:00:00Z",
                        "run_started_at": "2026-01-01T00:01:00Z",
                        "updated_at": "2026-01-01T00:09:00Z",
                        "html_url": "https://github.com/acme/project/actions/runs/30",
                        "url": "https://api.github.com/repos/acme/project/actions/runs/30",
                        "repository": { "full_name": "acme/project" },
                    },
                    {
                        "id": 31,
                        "workflow_id": 1,
                        "path": ".github/workflows/ci.yml",
                        "event": "push",
                        "head_sha": sha.as_str(),
                        "created_at": "2026-01-02T00:00:00Z",
                        "updated_at": "2026-01-02T00:09:00Z",
                        "html_url": "https://github.com/acme/project/actions/runs/31",
                        "repository": { "full_name": "acme/project" },
                        "status": null,
                        "conclusion": null,
                        "name": null,
                        "head_branch": null,
                        "run_started_at": null,
                    },
                ],
            })))
            .mount(&server)
            .await;
        let service = stand_in(&server).await;

        let listed = collect(&service, &WorkflowRunFilter::new()).await.unwrap();

        assert_eq!(
            listed,
            Collected::new(
                vec![
                    WorkflowRun::new(
                        NonZeroU64::new(30).unwrap(),
                        NonZeroU64::new(4).unwrap(),
                        ".github/workflows/release.yml",
                        "release",
                        sha.clone(),
                        "2026-01-01T00:00:00Z",
                        "2026-01-01T00:09:00Z",
                        "https://github.com/acme/project/actions/runs/30",
                        "acme/project",
                    )
                    .with_attempt(2)
                    .with_name("Release")
                    .with_status("completed")
                    .with_conclusion("success")
                    .with_head_branch("main")
                    .with_started_at("2026-01-01T00:01:00Z"),
                    WorkflowRun::new(
                        NonZeroU64::new(31).unwrap(),
                        NonZeroU64::new(1).unwrap(),
                        ".github/workflows/ci.yml",
                        "push",
                        sha,
                        "2026-01-02T00:00:00Z",
                        "2026-01-02T00:09:00Z",
                        "https://github.com/acme/project/actions/runs/31",
                        "acme/project",
                    ),
                ],
                true,
            )
        );
    }

    #[tokio::test]
    async fn a_run_at_a_malformed_commit_is_a_parse_error_not_a_shorter_list() {
        let refused = listing(with(
            run_row(2, &commit('a')),
            "head_sha",
            json!("not-a-commit"),
        ))
        .await;

        assert!(
            matches!(refused, Err(PullRequestError::Parse(_))),
            "{refused:?}"
        );
    }

    #[tokio::test]
    async fn a_run_without_its_identifier_fails_the_list() {
        let row = run_row(2, &commit('a'));
        for missing in [
            without(row.clone(), "id"),
            with(row.clone(), "id", json!(null)),
            with(row, "id", json!(0)),
        ] {
            let refused = listing(missing.clone()).await;

            assert!(unreadable(&refused), "{missing}: {refused:?}");
        }
    }

    #[tokio::test]
    async fn a_run_without_its_workflow_fails_the_list() {
        let row = run_row(2, &commit('a'));
        for missing in [
            without(row.clone(), "workflow_id"),
            with(row.clone(), "workflow_id", json!(null)),
            with(row, "workflow_id", json!(0)),
        ] {
            let refused = listing(missing.clone()).await;

            assert!(unreadable(&refused), "{missing}: {refused:?}");
        }
    }

    #[tokio::test]
    async fn a_run_without_a_field_github_always_sends_fails_the_list() {
        let row = run_row(2, &commit('a'));
        let mut missing: Vec<Value> = [
            "path",
            "event",
            "head_sha",
            "created_at",
            "updated_at",
            "html_url",
            "repository",
        ]
        .into_iter()
        .flat_map(|field| {
            [
                without(row.clone(), field),
                with(row.clone(), field, json!(null)),
            ]
        })
        .collect();
        missing.push(with(row, "repository", json!({})));

        for missing in missing {
            let refused = listing(missing.clone()).await;

            assert!(unreadable(&refused), "{missing}: {refused:?}");
        }
    }

    #[tokio::test]
    async fn a_run_attempt_that_does_not_fit_a_u32_fails_the_list() {
        let row = run_row(2, &commit('a'));

        let largest = listing(with(row.clone(), "run_attempt", json!(u32::MAX)))
            .await
            .unwrap();
        assert_eq!(largest.items[1].attempt, Some(u32::MAX));

        let past = with(row, "run_attempt", json!(u64::from(u32::MAX) + 1));
        let refused = listing(past.clone()).await;
        assert!(unreadable(&refused), "{past}: {refused:?}");
    }

    #[tokio::test]
    async fn a_job_keeps_githubs_words() {
        let server = MockServer::start().await;
        let sha = commit('c');
        Mock::given(method("GET"))
            .and(path(JOB))
            .and(header("authorization", "Bearer token"))
            .and(query_is(&[]))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": 9,
                "run_id": 30,
                "name": "test",
                "status": "completed",
                "conclusion": "failure",
                "head_sha": sha.as_str(),
                "html_url": "https://github.com/acme/project/actions/runs/30/job/9",
                "started_at": "2026-01-01T00:01:00Z",
                "completed_at": null,
            })))
            .mount(&server)
            .await;
        let service = stand_in(&server).await;

        let read = service
            .fetch_workflow_job(&project(&service), &token(), job())
            .await
            .unwrap();

        assert_eq!(
            read,
            WorkflowJob::new(
                job(),
                NonZeroU64::new(30).unwrap(),
                "test",
                "completed",
                sha
            )
            .with_conclusion("failure")
            .with_url("https://github.com/acme/project/actions/runs/30/job/9")
            .with_started_at("2026-01-01T00:01:00Z")
        );
    }

    #[tokio::test]
    async fn a_job_without_what_github_may_leave_out_is_read() {
        let sha = commit('c');

        let read = reading(job_row(&sha)).await.unwrap();

        assert_eq!(
            read,
            WorkflowJob::new(
                job(),
                NonZeroU64::new(30).unwrap(),
                "test",
                "completed",
                sha
            )
        );
    }

    #[tokio::test]
    async fn a_job_without_its_identifier_is_unreadable() {
        let row = job_row(&commit('c'));
        for missing in [without(row.clone(), "id"), with(row, "id", json!(0))] {
            let refused = reading(missing.clone()).await;

            assert!(unreadable(&refused), "{missing}: {refused:?}");
        }
    }

    #[tokio::test]
    async fn a_job_without_its_status_is_unreadable() {
        let row = job_row(&commit('c'));
        for missing in [
            without(row.clone(), "status"),
            with(row, "status", json!(null)),
        ] {
            let refused = reading(missing.clone()).await;

            assert!(unreadable(&refused), "{missing}: {refused:?}");
        }
    }

    #[tokio::test]
    async fn a_job_without_its_run_name_or_commit_is_unreadable() {
        let row = job_row(&commit('c'));
        let mut missing: Vec<Value> = ["run_id", "name", "head_sha"]
            .into_iter()
            .flat_map(|field| {
                [
                    without(row.clone(), field),
                    with(row.clone(), field, json!(null)),
                ]
            })
            .collect();
        missing.push(with(row, "run_id", json!(0)));

        for missing in missing {
            let refused = reading(missing.clone()).await;

            assert!(unreadable(&refused), "{missing}: {refused:?}");
        }
    }
}
