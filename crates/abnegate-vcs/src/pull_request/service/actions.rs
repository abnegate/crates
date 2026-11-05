use super::*;
use crate::commit_sha::CommitSha;
use crate::pull_request::JobLog;
use crate::pull_request::JobLogError;
use crate::pull_request::WorkflowJob;
use crate::pull_request::WorkflowRun;
use crate::pull_request::WorkflowRunFilter;
use crate::pull_request::github_workflow_job::GitHubWorkflowJob;
use crate::pull_request::github_workflow_run::GitHubWorkflowRun;
use crate::pull_request::github_workflow_runs::GitHubWorkflowRuns;
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

impl PullRequestService {
    /// Every workflow run in `repository` that `filter` admits, newest first,
    /// as GitHub lists them.
    ///
    /// The runs are read a hundred a page until a short page. GitHub lists
    /// at most 1,000 runs of any filter, so when more match, or when GitHub
    /// counted more than it listed, the answer is
    /// [`PullRequestError::SearchLimitReached`], never a partial list. A run
    /// whose commit is not a full commit identifier is
    /// [`PullRequestError::Parse`]. A run GitHub lists twice, as it may while
    /// a run is retried during the walk, is listed twice here.
    pub async fn list_workflow_runs(
        &self,
        repository: &Repository,
        token: &SecretValue,
        filter: &WorkflowRunFilter,
    ) -> PullRequestResult<Vec<WorkflowRun>> {
        let mut url = self.origin.endpoint(&[
            "repos",
            repository.owner(),
            repository.name(),
            "actions",
            "runs",
        ]);
        url.query_pairs_mut().extend_pairs(filter.pairs());

        let (rows, every) = walk(self, &url, token, |page: GitHubWorkflowRuns| {
            (page.total_count, page.workflow_runs)
        })
        .await?;
        if !every {
            return Err(PullRequestError::SearchLimitReached);
        }
        rows.into_iter().map(run).collect()
    }

    /// The workflow job GitHub identifies by `job` in `repository`.
    ///
    /// An answer without the job's identifier is
    /// [`PullRequestError::GitHubApi`], and one whose commit is not a full
    /// commit identifier is [`PullRequestError::Parse`]. Whether the job
    /// belongs to the run, commit or pull request a caller has in mind is the
    /// caller's to check.
    pub async fn fetch_workflow_job(
        &self,
        repository: &Repository,
        token: &SecretValue,
        job: NonZeroU64,
    ) -> PullRequestResult<WorkflowJob> {
        let job = job.to_string();
        let answered: GitHubWorkflowJob = self
            .get(
                self.origin.endpoint(&[
                    "repos",
                    repository.owner(),
                    repository.name(),
                    "actions",
                    "jobs",
                    &job,
                ]),
                token,
            )
            .await?;
        workflow_job(answered)
    }

    /// The first `limit` bytes of the log of the workflow job `job` in
    /// `repository`, and whether the log went on past them.
    ///
    /// GitHub answers the request for a log, which carries the token, with
    /// the log itself or with a redirect to where it stores it, under an
    /// address it signs. That redirect is followed once, and without the
    /// token, only to this service's own origin or over HTTPS to a host
    /// GitHub stores logs on or one under it, and never to an address that
    /// carries credentials; anywhere else is
    /// [`JobLogError::UnexpectedHost`], and a redirect that names nowhere is
    /// [`JobLogError::Undirected`]. Both requests together spend the one
    /// timeout a request is given.
    ///
    /// A refusal of the request for the log is reported as any refusal of
    /// this service's is. Where the log is stored, a status other than a
    /// success is [`JobLogError::Refused`], a download that could not be
    /// sent or ran out of time is [`JobLogError::Unreachable`], and a log
    /// that stops arriving part way through, from either, is
    /// [`JobLogError::Interrupted`]: never a shorter log. No error carries
    /// the signed address.
    pub async fn fetch_job_log(
        &self,
        repository: &Repository,
        token: &SecretValue,
        job: NonZeroU64,
        limit: usize,
    ) -> PullRequestResult<JobLog> {
        let deadline = Instant::now().checked_add(self.timeout);
        let job = job.to_string();
        let url = self.origin.endpoint(&[
            "repos",
            repository.owner(),
            repository.name(),
            "actions",
            "jobs",
            &job,
            "logs",
        ]);
        let response = sent(
            self.writer
                .request(Method::GET, url)
                .bearer_auth(token.expose())
                .header(header::ACCEPT, ACCEPT)
                .timeout(self.timeout),
        )
        .await?;

        let status = response.status();
        if status.is_success() {
            return logged(response, limit).await;
        }
        if !status.is_redirection() {
            return Err(refusal(response).await);
        }

        let target = destination(&self.origin, &response)?;
        let left = match deadline {
            Some(deadline) => remaining(deadline).ok_or(JobLogError::Unreachable)?,
            None => self.timeout,
        };
        let stored = self
            .writer
            .get(target)
            .timeout(left)
            .send()
            .await
            .map_err(|_| JobLogError::Unreachable)?;
        if !stored.status().is_success() {
            return Err(JobLogError::Refused(stored.status().as_u16()).into());
        }
        logged(stored, limit).await
    }
}

/// A workflow run GitHub listed, its commit parsed.
fn run(row: GitHubWorkflowRun) -> PullRequestResult<WorkflowRun> {
    Ok(WorkflowRun {
        id: row.id,
        workflow_id: row.workflow_id,
        attempt: row
            .run_attempt
            .and_then(|attempt| u32::try_from(attempt).ok()),
        name: row.name,
        path: row.path,
        event: row.event,
        status: row.status,
        conclusion: row.conclusion,
        head_branch: row.head_branch,
        head_sha: row.head_sha.as_deref().map(CommitSha::parse).transpose()?,
        created_at: row.created_at,
        started_at: row.run_started_at,
        updated_at: row.updated_at,
        url: row.html_url,
        repository: row
            .repository
            .map(|repository| repository.full_name)
            .unwrap_or_default(),
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
        head_sha: job.head_sha.as_deref().map(CommitSha::parse).transpose()?,
        url: job.html_url,
        started_at: job.started_at,
        completed_at: job.completed_at,
    })
}

/// The first `limit` bytes of a log `response` carries, and whether it went
/// on; a body that stops arriving is [`JobLogError::Interrupted`].
async fn logged(response: Response, limit: usize) -> PullRequestResult<JobLog> {
    let (content, truncated) = read_prefix(response, limit)
        .await
        .map_err(|_| JobLogError::Interrupted)?;
    Ok(JobLog::new(content, truncated))
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
/// HTTPS address on a host GitHub stores logs on, and carries no
/// credentials.
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

/// Whether `target` is an HTTPS address on one of [`LOG_HOSTS`] or a host
/// under one.
fn stores_logs(target: &Url) -> bool {
    target.scheme() == SECURE_SCHEME
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
    use crate::pull_request::service::fixtures::commit;
    use crate::pull_request::service::fixtures::project;
    use crate::pull_request::service::fixtures::stand_in;
    use crate::pull_request::service::fixtures::token;
    use serde_json::Value;
    use serde_json::json;
    use tokio::io::AsyncReadExt;
    use tokio::io::AsyncWriteExt;
    use tokio::net::TcpListener;
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

    /// `count` runs of one workflow at one commit, each with its own id.
    fn runs(count: usize, sha: &CommitSha) -> Vec<Value> {
        (1..=count)
            .map(|id| json!({ "id": id, "workflow_id": 1, "head_sha": sha.as_str() }))
            .collect()
    }

    /// A stand-in server on a loopback port that answers each request with
    /// what `answer` gives for its request line, and then hangs up; an empty
    /// answer hangs up without one.
    async fn hanging_up(answer: fn(&str) -> &'static str) -> PullRequestService {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a loopback port to listen on");
        let address = listener
            .local_addr()
            .expect("the address the listener is bound to");
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let mut request = Vec::new();
                let mut buffer = [0; 1024];
                while !request.windows(4).any(|end| end == b"\r\n\r\n") {
                    let read = stream.read(&mut buffer).await.unwrap_or_default();
                    if read == 0 {
                        break;
                    }
                    request.extend_from_slice(&buffer[..read]);
                }
                let head = String::from_utf8_lossy(&request);
                let line = head.lines().next().unwrap_or_default().to_string();
                let _ = stream.write_all(answer(&line).as_bytes()).await;
            }
        });
        PullRequestService::standing_in_for("github.com", &format!("http://{address}")).unwrap()
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

    #[tokio::test]
    async fn more_runs_than_github_lists_are_refused_whole() {
        let server = MockServer::start().await;
        let sha = commit('0');
        Mock::given(method("GET"))
            .and(path(RUNS))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                json!({ "total_count": 1001, "workflow_runs": runs(PAGE_SIZE, &sha) }),
            ))
            .expect(u64::try_from(MAXIMUM_PAGES).unwrap())
            .mount(&server)
            .await;
        let service = stand_in(&server).await;

        let refused = service
            .list_workflow_runs(
                &project(&service),
                &token(),
                &WorkflowRunFilter::new().with_head_sha(sha),
            )
            .await;

        assert!(
            matches!(refused, Err(PullRequestError::SearchLimitReached)),
            "{refused:?}"
        );
    }

    #[tokio::test]
    async fn the_run_filter_reaches_the_query_and_pages_are_followed_until_a_short_one() {
        let server = MockServer::start().await;
        let sha = commit('a');
        for (page, count) in [("1", PAGE_SIZE), ("2", 3)] {
            Mock::given(method("GET"))
                .and(path(RUNS))
                .and(query_param("head_sha", sha.as_str()))
                .and(query_param("event", "release"))
                .and(query_param("branch", "main"))
                .and(query_param("created", ">=2026-01-01"))
                .and(query_param("per_page", "100"))
                .and(query_param("page", page))
                .respond_with(ResponseTemplate::new(200).set_body_json(
                    json!({ "total_count": PAGE_SIZE + 3, "workflow_runs": runs(count, &sha) }),
                ))
                .expect(1)
                .mount(&server)
                .await;
        }
        let service = stand_in(&server).await;
        let filter = WorkflowRunFilter::new()
            .with_head_sha(sha.clone())
            .with_event("release")
            .with_branch("main")
            .with_created(">=2026-01-01");

        let listed = service
            .list_workflow_runs(&project(&service), &token(), &filter)
            .await
            .unwrap();

        assert_eq!(listed.len(), PAGE_SIZE + 3);
        assert!(listed.iter().all(|run| run.head_sha.as_ref() == Some(&sha)));
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
                        "workflow_id": 1,
                        "head_sha": sha.as_str(),
                        "status": "completed",
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

        let listed = service
            .list_workflow_runs(&project(&service), &token(), &WorkflowRunFilter::new())
            .await
            .unwrap();

        assert_eq!(
            listed,
            vec![
                WorkflowRun::new(30)
                    .with_workflow_id(4)
                    .with_attempt(2)
                    .with_name("Release")
                    .with_path(".github/workflows/release.yml")
                    .with_event("release")
                    .with_status("completed")
                    .with_conclusion("success")
                    .with_head_branch("main")
                    .with_head_sha(sha.clone())
                    .with_created_at("2026-01-01T00:00:00Z")
                    .with_started_at("2026-01-01T00:01:00Z")
                    .with_updated_at("2026-01-01T00:09:00Z")
                    .with_url("https://github.com/acme/project/actions/runs/30")
                    .with_repository("acme/project"),
                WorkflowRun::new(0)
                    .with_workflow_id(1)
                    .with_status("completed")
                    .with_head_sha(sha),
            ]
        );
    }

    #[tokio::test]
    async fn a_run_at_a_malformed_commit_is_a_parse_error_not_a_shorter_list() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(RUNS))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "workflow_runs": [{ "id": 1, "head_sha": "not-a-commit" }],
            })))
            .mount(&server)
            .await;
        let service = stand_in(&server).await;

        let refused = service
            .list_workflow_runs(&project(&service), &token(), &WorkflowRunFilter::new())
            .await;

        assert!(
            matches!(refused, Err(PullRequestError::Parse(_))),
            "{refused:?}"
        );
    }

    #[tokio::test]
    async fn a_job_keeps_githubs_words() {
        let server = MockServer::start().await;
        let sha = commit('c');
        Mock::given(method("GET"))
            .and(path(JOB))
            .and(header("authorization", "Bearer token"))
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
            WorkflowJob::new(9)
                .with_run_id(30)
                .with_name("test")
                .with_status("completed")
                .with_conclusion("failure")
                .with_head_sha(sha)
                .with_url("https://github.com/acme/project/actions/runs/30/job/9")
                .with_started_at("2026-01-01T00:01:00Z")
        );
    }

    #[tokio::test]
    async fn a_job_without_its_identifier_is_unreadable() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(JOB))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({ "run_id": 30, "status": "completed" })),
            )
            .mount(&server)
            .await;
        let service = stand_in(&server).await;

        let refused = service
            .fetch_workflow_job(&project(&service), &token(), job())
            .await;

        assert!(
            matches!(&refused, Err(PullRequestError::GitHubApi(said)) if said == UNREADABLE),
            "{refused:?}"
        );
    }
}
