use super::*;
use crate::commit_sha::CommitSha;
use crate::pull_request::ChecksOutcome;
use crate::pull_request::github_check_conclusion::GitHubCheckConclusion;
use crate::pull_request::github_check_run::GitHubCheckRun;
use crate::pull_request::github_check_runs::GitHubCheckRuns;
use crate::pull_request::github_check_status::GitHubCheckStatus;
use crate::pull_request::github_combined_status::GitHubCombinedStatus;
use crate::pull_request::github_commit_status::GitHubCommitStatus;
use crate::pull_request::github_status_state::GitHubStatusState;
use std::collections::BTreeSet;

/// Conclusions that fail a commit.
const FAILING_CONCLUSIONS: [GitHubCheckConclusion; 6] = [
    GitHubCheckConclusion::Failure,
    GitHubCheckConclusion::Error,
    GitHubCheckConclusion::Cancelled,
    GitHubCheckConclusion::TimedOut,
    GitHubCheckConclusion::ActionRequired,
    GitHubCheckConclusion::StartupFailure,
];

/// Conclusions that settle a run without failing the commit.
const PASSING_CONCLUSIONS: [GitHubCheckConclusion; 3] = [
    GitHubCheckConclusion::Success,
    GitHubCheckConclusion::Neutral,
    GitHubCheckConclusion::Skipped,
];

/// Commit status states that fail a commit.
const FAILING_STATES: [GitHubStatusState; 2] =
    [GitHubStatusState::Failure, GitHubStatusState::Error];

/// Commit status states that settle a context without failing the commit.
const PASSING_STATES: [GitHubStatusState; 1] = [GitHubStatusState::Success];

impl PullRequestService {
    /// What the check runs and commit statuses on `commit` add up to.
    ///
    /// Only each check's latest attempt counts, and both are read page after
    /// page, so a failure past the first page still fails the commit. A
    /// failure anywhere wins, named. Otherwise the outcome is a success only
    /// when every run and status said it passed: a run still going, one that
    /// finished stale, with a conclusion this crate does not know or with
    /// none, a status pending or in a state this crate does not know, and a
    /// read that could not reach every row all make it pending. A walk
    /// stopped at the page limit on a full page, or one that collected fewer
    /// rows than GitHub counted, is never a success. A neutral or skipped run
    /// fails nothing and counts as passing. Nothing reporting at all is
    /// [`ChecksOutcome::Absent`], whose meaning a caller decides: a new
    /// repository has no checks yet, and that is not a pass.
    ///
    /// A page longer than the 16 MiB this crate reads is
    /// [`PullRequestError::GitHubApi`], never a partial outcome and never
    /// [`ChecksOutcome::Absent`].
    pub async fn fetch_checks(
        &self,
        repository: &Repository,
        token: &SecretValue,
        commit: &CommitSha,
    ) -> PullRequestResult<ChecksOutcome> {
        let commits = [
            "repos",
            repository.owner(),
            repository.name(),
            "commits",
            commit.as_str(),
        ];
        let mut runs = self
            .origin
            .endpoint(&[&commits[..], &["check-runs"]].concat());
        runs.query_pairs_mut().append_pair("filter", "latest");
        let statuses = self.origin.endpoint(&[&commits[..], &["status"]].concat());

        let ((runs, every_run), (statuses, every_status)) = tokio::try_join!(
            walk(self, &runs, token, |page: GitHubCheckRuns| {
                (page.total_count, page.check_runs)
            }),
            walk(self, &statuses, token, |page: GitHubCombinedStatus| {
                (page.total_count, page.statuses)
            }),
        )?;

        Ok(fold(&runs, &statuses, every_run && every_status))
    }
}

/// Every row a paged answer at `url` holds, page after page until a short
/// one, and whether that is all of them.
///
/// It is not when the walk stopped at [`MAXIMUM_PAGES`] on a full page, or
/// when it collected fewer rows than the largest count a page gave.
async fn walk<P: DeserializeOwned, R>(
    service: &PullRequestService,
    url: &Url,
    token: &SecretValue,
    rows: impl Fn(P) -> (Option<u64>, Vec<R>),
) -> PullRequestResult<(Vec<R>, bool)> {
    let mut collected: Vec<R> = Vec::new();
    let mut total: Option<u64> = None;

    for page in 1..=MAXIMUM_PAGES {
        let mut paged = url.clone();
        paged
            .query_pairs_mut()
            .append_pair("per_page", &PAGE_SIZE.to_string())
            .append_pair("page", &page.to_string());
        let (count, batch) = rows(service.get(paged, token).await?);
        total = total.max(count);
        let short = batch.len() < PAGE_SIZE;
        collected.extend(batch);
        if short {
            let every = total.is_none_or(|counted| {
                u64::try_from(collected.len()).is_ok_and(|read| counted <= read)
            });
            return Ok((collected, every));
        }
    }

    Ok((collected, false))
}

/// What the runs and statuses read add up to: every failure, named, ahead of
/// anything that has not said it passed or was not read, ahead of silence,
/// ahead of success.
fn fold(runs: &[GitHubCheckRun], statuses: &[GitHubCommitStatus], every: bool) -> ChecksOutcome {
    let failed: BTreeSet<&str> = runs
        .iter()
        .filter(|run| concluded(run, &FAILING_CONCLUSIONS))
        .map(|run| run.name.as_str())
        .chain(
            statuses
                .iter()
                .filter(|status| FAILING_STATES.contains(&status.state))
                .map(|status| status.context.as_str()),
        )
        .collect();
    let passed = every
        && runs.iter().all(|run| concluded(run, &PASSING_CONCLUSIONS))
        && statuses
            .iter()
            .all(|status| PASSING_STATES.contains(&status.state));

    if !failed.is_empty() {
        ChecksOutcome::Failure(failed.into_iter().map(str::to_string).collect())
    } else if !passed {
        ChecksOutcome::Pending
    } else if runs.is_empty() && statuses.is_empty() {
        ChecksOutcome::Absent
    } else {
        ChecksOutcome::Success
    }
}

/// Whether `run` finished with one of `conclusions`.
fn concluded(run: &GitHubCheckRun, conclusions: &[GitHubCheckConclusion]) -> bool {
    run.status == GitHubCheckStatus::Completed
        && run
            .conclusion
            .is_some_and(|conclusion| conclusions.contains(&conclusion))
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
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::header;
    use wiremock::matchers::method;
    use wiremock::matchers::path;
    use wiremock::matchers::query_param;
    use wiremock::matchers::query_param_is_missing;

    fn runs(sha: &CommitSha) -> String {
        format!("/repos/acme/project/commits/{sha}/check-runs")
    }

    fn statuses(sha: &CommitSha) -> String {
        format!("/repos/acme/project/commits/{sha}/status")
    }

    fn answer(body: Value) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(body)
    }

    /// `count` runs that finished and passed, each under its own name.
    fn passing(count: usize) -> Vec<Value> {
        (0..count)
            .map(|index| {
                json!({ "name": format!("check-{index}"), "status": "completed", "conclusion": "success" })
            })
            .collect()
    }

    async fn answering(server: &MockServer, route: String, body: Value) {
        Mock::given(method("GET"))
            .and(path(route))
            .respond_with(answer(body))
            .mount(server)
            .await;
    }

    async fn outcome(server: &MockServer, sha: &CommitSha) -> ChecksOutcome {
        let service = stand_in(server).await;
        service
            .fetch_checks(&project(&service), &token(), sha)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn checks_fold_runs_and_statuses_into_one_outcome() {
        let server = MockServer::start().await;
        let sha = commit('0');
        answering(
            &server,
            runs(&sha),
            json!({
                "check_runs": [
                    { "name": "build", "status": "completed", "conclusion": "success" },
                    { "name": "lint", "status": "completed", "conclusion": "skipped" },
                    { "name": "test", "status": "in_progress", "conclusion": null },
                ]
            }),
        )
        .await;
        answering(
            &server,
            statuses(&sha),
            json!({
                "state": "pending",
                "statuses": [{ "context": "ci/deploy", "state": "success" }],
            }),
        )
        .await;

        assert_eq!(
            outcome(&server, &sha).await,
            ChecksOutcome::Pending,
            "a run still going is pending"
        );
    }

    #[tokio::test]
    async fn checks_follow_every_page_before_judging() {
        let server = MockServer::start().await;
        let sha = commit('3');
        Mock::given(method("GET"))
            .and(path(runs(&sha)))
            .and(query_param("page", "1"))
            .respond_with(answer(json!({ "check_runs": passing(PAGE_SIZE) })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(runs(&sha)))
            .and(query_param("page", "2"))
            .respond_with(answer(json!({
                "check_runs": [{ "name": "deploy", "status": "completed", "conclusion": "failure" }]
            })))
            .mount(&server)
            .await;
        answering(
            &server,
            statuses(&sha),
            json!({ "state": "success", "statuses": [] }),
        )
        .await;

        assert_eq!(
            outcome(&server, &sha).await,
            ChecksOutcome::Failure(vec!["deploy".to_string()]),
            "the failure on the second page decides the outcome"
        );
    }

    #[tokio::test]
    async fn a_failed_status_names_itself_and_silence_is_absent() {
        let server = MockServer::start().await;
        let failing = commit('1');
        let silent = commit('2');

        for (sha, runs_answer, statuses_answer) in [
            (
                &failing,
                json!({ "check_runs": [{ "name": "test", "status": "completed", "conclusion": "failure" }] }),
                json!({ "state": "failure", "statuses": [{ "context": "ci/deploy", "state": "error" }] }),
            ),
            (
                &silent,
                json!({ "check_runs": [] }),
                json!({ "state": "pending", "statuses": [] }),
            ),
        ] {
            answering(&server, runs(sha), runs_answer).await;
            answering(&server, statuses(sha), statuses_answer).await;
        }

        assert_eq!(
            outcome(&server, &failing).await,
            ChecksOutcome::Failure(vec!["ci/deploy".to_string(), "test".to_string()])
        );
        assert_eq!(outcome(&server, &silent).await, ChecksOutcome::Absent);
    }

    #[tokio::test]
    async fn statuses_follow_every_page_too() {
        let server = MockServer::start().await;
        let sha = commit('4');
        let passed: Vec<Value> = (0..PAGE_SIZE)
            .map(|index| json!({ "context": format!("ci/{index}"), "state": "success" }))
            .collect();
        answering(
            &server,
            runs(&sha),
            json!({ "total_count": 0, "check_runs": [] }),
        )
        .await;
        Mock::given(method("GET"))
            .and(path(statuses(&sha)))
            .and(query_param("per_page", "100"))
            .and(query_param("page", "1"))
            .respond_with(answer(json!({
                "state": "failure",
                "total_count": PAGE_SIZE + 1,
                "statuses": passed,
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(statuses(&sha)))
            .and(query_param("per_page", "100"))
            .and(query_param("page", "2"))
            .respond_with(answer(json!({
                "state": "failure",
                "total_count": PAGE_SIZE + 1,
                "statuses": [{ "context": "ci/deploy", "state": "failure" }],
            })))
            .expect(1)
            .mount(&server)
            .await;

        assert_eq!(
            outcome(&server, &sha).await,
            ChecksOutcome::Failure(vec!["ci/deploy".to_string()]),
            "the failure on the second page of statuses decides the outcome"
        );
    }

    #[tokio::test]
    async fn a_capped_walk_is_never_reported_as_success() {
        let server = MockServer::start().await;
        let capped = commit('5');
        let failing = commit('6');
        let runs_short = commit('7');
        let statuses_short = commit('8');
        let unsent = commit('9');

        Mock::given(method("GET"))
            .and(path(runs(&capped)))
            .respond_with(answer(json!({ "check_runs": passing(PAGE_SIZE) })))
            .expect(u64::try_from(MAXIMUM_PAGES).unwrap())
            .mount(&server)
            .await;
        answering(&server, statuses(&capped), json!({ "statuses": [] })).await;

        let mut timed_out = passing(PAGE_SIZE - 1);
        timed_out
            .push(json!({ "name": "deploy", "status": "completed", "conclusion": "timed_out" }));
        answering(&server, runs(&failing), json!({ "check_runs": timed_out })).await;
        answering(&server, statuses(&failing), json!({ "statuses": [] })).await;

        answering(
            &server,
            runs(&runs_short),
            json!({ "total_count": 3, "check_runs": passing(2) }),
        )
        .await;
        answering(
            &server,
            statuses(&runs_short),
            json!({ "total_count": 0, "statuses": [] }),
        )
        .await;

        answering(
            &server,
            runs(&statuses_short),
            json!({ "total_count": 1, "check_runs": passing(1) }),
        )
        .await;
        answering(
            &server,
            statuses(&statuses_short),
            json!({
                "total_count": 2,
                "statuses": [{ "context": "ci/deploy", "state": "success" }],
            }),
        )
        .await;

        answering(
            &server,
            runs(&unsent),
            json!({ "total_count": 5, "check_runs": [] }),
        )
        .await;
        answering(
            &server,
            statuses(&unsent),
            json!({ "total_count": 0, "statuses": [] }),
        )
        .await;

        for (sha, expected, why) in [
            (
                &capped,
                ChecksOutcome::Pending,
                "a walk stopped at the page limit on a full page may have missed a failure",
            ),
            (
                &failing,
                ChecksOutcome::Failure(vec!["deploy".to_string()]),
                "a failure read before the page limit still fails the commit",
            ),
            (
                &runs_short,
                ChecksOutcome::Pending,
                "fewer runs than GitHub counted are not every run",
            ),
            (
                &statuses_short,
                ChecksOutcome::Pending,
                "fewer statuses than GitHub counted are not every status",
            ),
            (
                &unsent,
                ChecksOutcome::Pending,
                "runs GitHub counted but did not send are not silence",
            ),
        ] {
            assert_eq!(outcome(&server, sha).await, expected, "{why}");
        }
    }

    #[tokio::test]
    async fn a_commit_whose_runs_only_skipped_still_counts_as_reported() {
        let server = MockServer::start().await;
        let sha = commit('b');
        answering(
            &server,
            runs(&sha),
            json!({
                "total_count": 2,
                "check_runs": [
                    { "name": "lint", "status": "completed", "conclusion": "skipped" },
                    { "name": "docs", "status": "completed", "conclusion": "neutral" },
                ],
            }),
        )
        .await;
        answering(
            &server,
            statuses(&sha),
            json!({ "state": "pending", "total_count": 0, "statuses": [] }),
        )
        .await;

        assert_eq!(
            outcome(&server, &sha).await,
            ChecksOutcome::Success,
            "a skipped or neutral run fails nothing, but it did report"
        );
    }

    #[tokio::test]
    async fn check_runs_are_asked_for_their_latest_attempt_only() {
        let server = MockServer::start().await;
        let sha = commit('c');
        Mock::given(method("GET"))
            .and(path(runs(&sha)))
            .and(query_param_is_missing("filter"))
            .respond_with(answer(json!({
                "check_runs": [{ "name": "test", "status": "completed", "conclusion": "failure" }]
            })))
            .expect(0)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(runs(&sha)))
            .and(header("authorization", "Bearer token"))
            .and(query_param("filter", "latest"))
            .and(query_param("per_page", "100"))
            .and(query_param("page", "1"))
            .respond_with(answer(json!({
                "total_count": 1,
                "check_runs": [{ "name": "test", "status": "completed", "conclusion": "success" }],
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(statuses(&sha)))
            .and(header("authorization", "Bearer token"))
            .and(query_param_is_missing("filter"))
            .and(query_param("per_page", "100"))
            .and(query_param("page", "1"))
            .respond_with(answer(json!({ "total_count": 0, "statuses": [] })))
            .expect(1)
            .mount(&server)
            .await;

        assert_eq!(
            outcome(&server, &sha).await,
            ChecksOutcome::Success,
            "an earlier attempt that failed and was rerun does not fail the commit"
        );
    }

    #[tokio::test]
    async fn an_answer_without_a_total_is_judged_on_its_rows() {
        let server = MockServer::start().await;
        let sha = commit('d');
        Mock::given(method("GET"))
            .and(path(runs(&sha)))
            .respond_with(answer(json!({ "check_runs": passing(3) })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(statuses(&sha)))
            .respond_with(answer(json!({
                "statuses": [{ "context": "ci/deploy", "state": "success" }]
            })))
            .expect(1)
            .mount(&server)
            .await;

        assert_eq!(
            outcome(&server, &sha).await,
            ChecksOutcome::Success,
            "a short page with no count is every row there is"
        );
    }

    #[test]
    fn a_run_or_status_that_never_said_it_passed_fails_nothing_and_is_pending() {
        let unsettled = [
            json!({ "name": "build", "status": "completed", "conclusion": "stale" }),
            json!({ "name": "build", "status": "completed", "conclusion": "superseded" }),
            json!({ "name": "build", "status": "completed", "conclusion": null }),
            json!({ "name": "build", "status": "completed" }),
            json!({ "name": "build", "status": "deferred", "conclusion": null }),
        ];
        for run in unsettled {
            let runs: Vec<GitHubCheckRun> = serde_json::from_value(json!([run])).unwrap();
            let failing: Vec<GitHubCheckRun> = serde_json::from_value(json!([
                run,
                { "name": "test", "status": "completed", "conclusion": "failure" },
            ]))
            .unwrap();

            assert_eq!(fold(&runs, &[], true), ChecksOutcome::Pending, "{run}");
            assert_eq!(
                fold(&failing, &[], true),
                ChecksOutcome::Failure(vec!["test".to_string()]),
                "{run}"
            );
        }

        let unknown: Vec<GitHubCommitStatus> = serde_json::from_value(json!([
            { "context": "ci/deploy", "state": "abandoned" },
        ]))
        .unwrap();
        assert_eq!(fold(&[], &unknown, true), ChecksOutcome::Pending);
    }

    /// A check-runs page with no runs, padded to exactly `length` bytes with a
    /// sentinel no error may repeat.
    fn padded(length: usize) -> String {
        let opening = r#"{"total_count":0,"check_runs":[],"padding":"RAW-SENTINEL"#;
        let closing = r#""}"#;
        let padding = "x".repeat(length - opening.len() - closing.len());
        format!("{opening}{padding}{closing}")
    }

    #[tokio::test]
    async fn a_check_runs_page_past_the_answer_bound_is_an_error_and_never_absent() {
        let server = MockServer::start().await;
        let sha = commit('f');
        Mock::given(method("GET"))
            .and(path(runs(&sha)))
            .respond_with(
                ResponseTemplate::new(200).set_body_string(padded(MAXIMUM_ANSWER_BYTES + 1)),
            )
            .expect(1)
            .mount(&server)
            .await;
        answering(
            &server,
            statuses(&sha),
            json!({ "total_count": 0, "statuses": [] }),
        )
        .await;
        let service = stand_in(&server).await;

        let read = service
            .fetch_checks(&project(&service), &token(), &sha)
            .await;

        assert!(
            matches!(read, Err(PullRequestError::GitHubApi(ref text)) if text == OVERSIZED),
            "a page too long to read is not an empty one: {read:?}"
        );
        assert!(!format!("{read:?}").contains("RAW-SENTINEL"), "{read:?}");
    }

    #[tokio::test]
    async fn a_commit_whose_only_run_is_stale_is_pending() {
        let server = MockServer::start().await;
        let sha = commit('e');
        answering(
            &server,
            runs(&sha),
            json!({
                "total_count": 1,
                "check_runs": [{ "name": "build", "status": "completed", "conclusion": "stale" }],
            }),
        )
        .await;
        answering(
            &server,
            statuses(&sha),
            json!({ "total_count": 0, "statuses": [] }),
        )
        .await;

        assert_eq!(
            outcome(&server, &sha).await,
            ChecksOutcome::Pending,
            "a run GitHub marked stale never said it passed"
        );
    }
}
