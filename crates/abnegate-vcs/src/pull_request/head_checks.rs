use crate::commit_sha::CommitSha;
use crate::pull_request::CheckRun;
use crate::pull_request::Collected;
use crate::pull_request::CommitStatus;
use crate::pull_request::WorkflowRun;

/// What checked the commit at the tip of a pull request's head: its GitHub
/// Actions runs, the latest attempt of each check run, and the latest status
/// each context reported, each list saying whether it holds every row.
///
/// Read through GitHub's GraphQL API, each row holds what GitHub's REST API
/// holds for it, but for what GraphQL does not say:
///
/// - a [`WorkflowRun`]'s `attempt` and `started_at` are `None`;
/// - its `path` is empty where GraphQL names no file, as for a dynamic
///   workflow such as CodeQL's default setup or the Copilot coding agent,
///   which REST names under `dynamic/`;
/// - its `name` is its workflow's, where REST gives the run's own, which
///   differs for a dynamic workflow's run or one a `run-name` names;
/// - its `head_branch` is the branch its check suite names or, where the
///   suite names none, as for a pull request from a fork, the pull request's
///   head branch, as REST names it; a dynamic run whose suite names none,
///   such as code scanning's, has none, where REST names the pull request's
///   ref;
/// - its `repository` is the name the caller gave the repository, not
///   GitHub's spelling of it;
/// - its `status` and `conclusion` are its check suite's, the newest suite's
///   when the run was re-run.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct HeadChecks {
    /// The commit checked.
    pub sha: CommitSha,
    /// One entry per workflow run on the commit in the pull request's own
    /// repository, the newest run first.
    pub workflow_runs: Collected<WorkflowRun>,
    /// The latest attempt of every check run on the commit in the pull
    /// request's own repository.
    pub check_runs: Collected<CheckRun>,
    /// The latest status every context reported on the commit.
    pub statuses: Collected<CommitStatus>,
}

impl HeadChecks {
    /// `workflow_runs`, `check_runs` and `statuses` on the commit `sha`.
    pub fn new(
        sha: CommitSha,
        workflow_runs: Collected<WorkflowRun>,
        check_runs: Collected<CheckRun>,
        statuses: Collected<CommitStatus>,
    ) -> Self {
        Self {
            sha,
            workflow_runs,
            check_runs,
            statuses,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn head_checks_hold_what_they_were_made_from() {
        let sha = CommitSha::parse(&"a".repeat(40)).unwrap();
        let checks = HeadChecks::new(
            sha.clone(),
            Collected::new(Vec::new(), true),
            Collected::new(vec![CheckRun::new("completed")], false),
            Collected::new(vec![CommitStatus::new("ci/deploy", "success")], true),
        );

        assert_eq!(checks.sha, sha);
        assert!(checks.workflow_runs.complete);
        assert_eq!(checks.check_runs.items, vec![CheckRun::new("completed")]);
        assert!(!checks.check_runs.complete);
        assert_eq!(checks.statuses.items.len(), 1);
    }
}
