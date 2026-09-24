use super::*;
use crate::commit_sha::CommitSha;
use crate::pull_request::ChangedFile;
use crate::pull_request::PullRequestDetail;
use crate::pull_request::github_pull_request_full::GitHubPullRequestFull;

impl PullRequestService {
    /// Read a pull request back as GitHub describes it now.
    ///
    /// Its branches and commits are parsed rather than taken on trust: a head,
    /// base or commit GitHub answers with that this crate does not accept is
    /// refused as [`PullRequestError::Parse`], never read as something else.
    /// Until GitHub has worked them out, whether it merges is
    /// [`Mergeability::Unknown`] and what stands in its way is
    /// [`crate::pull_request::MergeableState::Unknown`].
    pub async fn fetch_pull(
        &self,
        reference: &PullRequestReference,
        token: &SecretValue,
    ) -> PullRequestResult<PullRequestDetail> {
        let number = reference.number().to_string();
        let repository = reference.repository();
        let pull: GitHubPullRequestFull = self
            .get(
                self.origin.endpoint(&[
                    "repos",
                    repository.owner(),
                    repository.name(),
                    "pulls",
                    &number,
                ]),
                token,
            )
            .await?;
        detail(pull)
    }

    /// Every file a pull request changes, in the order GitHub lists them.
    ///
    /// Files are read a hundred at a time for at most ten pages, so a pull
    /// request that changes more than a thousand files is listed only that
    /// far. A page longer than the 16 MiB this crate reads is
    /// [`PullRequestError::GitHubApi`], never a partial or empty list.
    pub async fn fetch_files(
        &self,
        reference: &PullRequestReference,
        token: &SecretValue,
    ) -> PullRequestResult<Vec<ChangedFile>> {
        let number = reference.number().to_string();
        let repository = reference.repository();
        self.get_all(
            &[
                "repos",
                repository.owner(),
                repository.name(),
                "pulls",
                &number,
                "files",
            ],
            token,
        )
        .await
    }
}

/// The pull request GitHub answered with, its branches and commits parsed.
fn detail(pull: GitHubPullRequestFull) -> PullRequestResult<PullRequestDetail> {
    Ok(PullRequestDetail {
        node_id: pull.node_id,
        number: pull.number,
        title: pull.title,
        body: pull.body,
        state: pull.state,
        draft: pull.draft,
        merged: pull.merged,
        merge_commit_sha: pull
            .merge_commit_sha
            .as_deref()
            .map(CommitSha::parse)
            .transpose()?,
        head: BranchName::parse(&pull.head.reference)?,
        head_sha: CommitSha::parse(&pull.head.sha)?,
        base: BranchName::parse(&pull.base.reference)?,
        mergeable: Mergeability::from_flag(pull.mergeable),
        mergeable_state: pull.mergeable_state.unwrap_or_default(),
        changed_files: pull.changed_files,
        additions: pull.additions,
        deletions: pull.deletions,
        commits: pull.commits,
        url: pull.html_url,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse_error::ParseError;
    use crate::pull_request::FileStatus;
    use crate::pull_request::MergeableState;
    use crate::pull_request::PullRequestState;
    use crate::pull_request::service::fixtures::commit;
    use crate::pull_request::service::fixtures::seven;
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

    const PULL: &str = "/repos/acme/project/pulls/7";

    const FILES: &str = "/repos/acme/project/pulls/7/files";

    fn whole() -> Value {
        json!({
            "node_id": "PR_kwDOAcme7",
            "number": 7,
            "title": "(feat): basket totals",
            "body": "Adds totals to the basket.",
            "state": "open",
            "draft": true,
            "merged": false,
            "merge_commit_sha": commit('c').as_str(),
            "head": { "ref": "task/one", "sha": commit('a').as_str() },
            "base": { "ref": "main", "sha": commit('b').as_str() },
            "html_url": "https://github.com/acme/project/pull/7",
            "mergeable": false,
            "mergeable_state": "dirty",
            "changed_files": 3,
            "additions": 120,
            "deletions": 14,
            "commits": 2,
        })
    }

    async fn answering(answer: Value) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(PULL))
            .respond_with(ResponseTemplate::new(200).set_body_json(answer))
            .mount(&server)
            .await;
        server
    }

    async fn read(answer: Value) -> PullRequestResult<PullRequestDetail> {
        let server = answering(answer).await;
        let service = stand_in(&server).await;
        service.fetch_pull(&seven(&service), &token()).await
    }

    fn changed(filename: &str, status: FileStatus, additions: u32) -> ChangedFile {
        ChangedFile {
            filename: filename.to_string(),
            status,
            additions,
            deletions: 1,
        }
    }

    #[tokio::test]
    async fn a_pull_request_is_read_back_whole() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(PULL))
            .and(header("authorization", "Bearer token"))
            .and(header("accept", ACCEPT))
            .respond_with(ResponseTemplate::new(200).set_body_json(whole()))
            .expect(1)
            .mount(&server)
            .await;
        let service = stand_in(&server).await;

        let pull = service
            .fetch_pull(&seven(&service), &token())
            .await
            .unwrap();

        assert_eq!(
            pull,
            PullRequestDetail {
                node_id: "PR_kwDOAcme7".to_string(),
                number: NonZeroU64::new(7).unwrap(),
                title: "(feat): basket totals".to_string(),
                body: Some("Adds totals to the basket.".to_string()),
                state: PullRequestState::Open,
                draft: true,
                merged: false,
                merge_commit_sha: Some(commit('c')),
                head: BranchName::parse("task/one").unwrap(),
                head_sha: commit('a'),
                base: BranchName::parse("main").unwrap(),
                mergeable: Mergeability::Conflicted,
                mergeable_state: MergeableState::Dirty,
                changed_files: 3,
                additions: 120,
                deletions: 14,
                commits: 2,
                url: "https://github.com/acme/project/pull/7".to_string(),
            }
        );
    }

    #[tokio::test]
    async fn a_pull_request_github_has_not_finished_computing_reads_as_unknown() {
        for unfinished in [None, Some(Value::Null), Some(json!("unknown"))] {
            let mut answer = whole();
            answer["mergeable"] = Value::Null;
            answer["merge_commit_sha"] = Value::Null;
            match &unfinished {
                Some(state) => answer["mergeable_state"] = state.clone(),
                None => {
                    answer.as_object_mut().unwrap().remove("mergeable_state");
                }
            }

            let pull = read(answer).await.unwrap();

            assert_eq!(pull.mergeable, Mergeability::Unknown, "{unfinished:?}");
            assert!(!pull.mergeable.conflicted(), "{unfinished:?}");
            assert_eq!(
                pull.mergeable_state,
                MergeableState::Unknown,
                "{unfinished:?}"
            );
            assert_eq!(pull.merge_commit_sha, None, "{unfinished:?}");
        }
    }

    #[tokio::test]
    async fn what_a_smaller_answer_leaves_out_reads_as_nothing_rather_than_failing() {
        let mut answer = whole();
        let fields = answer.as_object_mut().unwrap();
        for optional in [
            "body",
            "draft",
            "merged",
            "merge_commit_sha",
            "mergeable",
            "mergeable_state",
            "changed_files",
            "additions",
            "deletions",
            "commits",
        ] {
            fields.remove(optional);
        }

        let pull = read(answer).await.unwrap();

        assert_eq!(pull.body, None);
        assert!(!pull.draft);
        assert!(!pull.merged);
        assert_eq!(pull.merge_commit_sha, None);
        assert_eq!(pull.mergeable, Mergeability::Unknown);
        assert_eq!(pull.mergeable_state, MergeableState::Unknown);
        assert_eq!(
            (
                pull.changed_files,
                pull.additions,
                pull.deletions,
                pull.commits
            ),
            (0, 0, 0, 0)
        );
        assert_eq!(pull.head_sha, commit('a'));
    }

    #[tokio::test]
    async fn a_pull_request_with_an_unreadable_head_is_refused_rather_than_guessed() {
        for (field, unreadable) in [
            ("/head/ref", "task/../main"),
            ("/head/ref", ""),
            ("/base/ref", "-main"),
            ("/head/sha", "abc123"),
            ("/head/sha", ""),
            ("/merge_commit_sha", "not-a-commit"),
        ] {
            let mut answer = whole();
            *answer.pointer_mut(field).unwrap() = json!(unreadable);

            let failure = read(answer)
                .await
                .expect_err("an unreadable branch or commit must not be read as another one");

            let refused = match failure {
                PullRequestError::Parse(ParseError::BranchName(value)) => value,
                PullRequestError::Parse(ParseError::CommitSha(value)) => value,
                other => panic!("{field} = {unreadable:?} gave {other:?}"),
            };
            assert_eq!(refused, unreadable, "{field}");
        }
    }

    #[tokio::test]
    async fn a_pull_request_missing_what_identifies_it_is_refused() {
        for required in [
            "/node_id",
            "/number",
            "/title",
            "/state",
            "/head",
            "/base",
            "/html_url",
            "/head/ref",
            "/head/sha",
        ] {
            let mut answer = whole();
            let (parent, field) = required.rsplit_once('/').unwrap();
            answer
                .pointer_mut(parent)
                .and_then(Value::as_object_mut)
                .unwrap()
                .remove(field);

            let failure = read(answer)
                .await
                .expect_err("a pull request with a part missing must not be filled in");

            assert!(
                matches!(&failure, PullRequestError::GitHubApi(message) if message == UNREADABLE),
                "{required} missing gave {failure:?}"
            );
        }
    }

    #[tokio::test]
    async fn every_changed_file_is_listed_across_pages() {
        let mut first: Vec<Value> = (0..100)
            .map(|index| {
                json!({
                    "filename": format!("src/file{index}.rs"),
                    "status": "modified",
                    "additions": index,
                    "deletions": 1,
                })
            })
            .collect();
        first[0] = json!({
            "filename": "src/basket.rs",
            "previous_filename": "src/cart.rs",
            "status": "renamed",
            "additions": 0,
            "deletions": 1,
        });
        first[1] = json!({
            "filename": "src/file1.rs",
            "additions": 1,
            "deletions": 1,
        });
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(FILES))
            .and(header("authorization", "Bearer token"))
            .and(query_param("per_page", "100"))
            .and(query_param("page", "1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(first))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(FILES))
            .and(query_param("per_page", "100"))
            .and(query_param("page", "2"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([{
                "filename": "assets/logo.svg",
                "status": "type_changed",
                "additions": 100,
                "deletions": 1,
            }])))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(FILES))
            .and(query_param("page", "3"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
            .expect(0)
            .mount(&server)
            .await;
        let service = stand_in(&server).await;

        let files = service
            .fetch_files(&seven(&service), &token())
            .await
            .unwrap();

        assert_eq!(files.len(), 101);
        assert_eq!(
            files[0],
            changed("src/basket.rs", FileStatus::Renamed, 0),
            "a renamed file is listed under its new name"
        );
        assert_eq!(
            files[1],
            changed("src/file1.rs", FileStatus::Unknown, 1),
            "a file GitHub gives no status for is listed as unknown"
        );
        assert_eq!(
            files[99],
            changed("src/file99.rs", FileStatus::Modified, 99)
        );
        assert_eq!(
            files[100],
            changed("assets/logo.svg", FileStatus::Unknown, 100),
            "a status this crate does not know yet is listed as unknown"
        );
    }

    #[tokio::test]
    async fn a_pull_request_that_cannot_be_seen_is_not_found() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(404).set_body_json(json!({ "message": "Not Found" })),
            )
            .expect(2)
            .mount(&server)
            .await;
        let service = stand_in(&server).await;
        let reference = seven(&service);

        let pull = service
            .fetch_pull(&reference, &token())
            .await
            .expect_err("a pull request the token cannot see must not read as an empty one");
        let files = service
            .fetch_files(&reference, &token())
            .await
            .expect_err("files the token cannot see must not read as none");

        assert!(matches!(pull, PullRequestError::NotFound), "{pull:?}");
        assert!(matches!(files, PullRequestError::NotFound), "{files:?}");
    }
}
