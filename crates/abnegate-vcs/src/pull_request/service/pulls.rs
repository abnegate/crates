use super::*;
use crate::commit_sha::CommitSha;
use crate::pull_request::ChangedFile;
use crate::pull_request::FoundPullRequest;
use crate::pull_request::ListedPullRequest;
use crate::pull_request::Page;
use crate::pull_request::PullRequestDetail;
use crate::pull_request::PullRequestState;
use crate::pull_request::StateFilter;
use crate::pull_request::github_listed_branch::GitHubListedBranch;
use crate::pull_request::github_listed_pull::GitHubListedPull;
use crate::pull_request::github_pull::GitHubPull;
use crate::pull_request::repository::named;

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
        let pull: GitHubPull = self
            .get(
                self.repository_endpoint(repository, &["pulls", &number])?,
                token,
            )
            .await?;
        detail(pull)
    }

    /// The pull request from `head` into `base`: the open one if there is
    /// one, or else the one most recently opened, whether it was closed or
    /// merged. `None` when there is none.
    ///
    /// `head` is a branch of the repository itself, or, when `fork` names its
    /// owner, of that owner's fork. GitHub is asked for both branches, and
    /// only a pull request whose head is that owner's `head` and whose base is
    /// `base` is taken, whatever else GitHub's answer holds. A `fork` owner
    /// GitHub does not allow is [`PullRequestError::InvalidRepositoryName`].
    /// Only the hundred pull requests GitHub lists first, newest first, are
    /// looked through.
    pub async fn find_pull_request(
        &self,
        repository: &Repository,
        token: &SecretValue,
        head: &BranchName,
        base: &BranchName,
        fork: Option<&str>,
    ) -> PullRequestResult<Option<FoundPullRequest>> {
        let owner = owned(fork.unwrap_or(repository.owner()))?;
        let mut url = self.repository_endpoint(repository, &["pulls"])?;
        url.query_pairs_mut()
            .append_pair("head", &format!("{owner}:{head}"))
            .append_pair("base", base.as_str())
            .append_pair("state", "all")
            .append_pair("sort", "created")
            .append_pair("direction", "desc")
            .append_pair("per_page", &PAGE_SIZE.to_string());

        let listed: Vec<GitHubListedPull> = self.get(url, token).await?;
        let mut matching = listed
            .into_iter()
            .filter(|pull| pull.base.reference == base.as_str() && headed(&pull.head, owner, head));
        let first = matching.next();
        let chosen = match first.as_ref().map(|pull| pull.state) {
            Some(PullRequestState::Open) | None => first,
            Some(_) => matching
                .find(|pull| pull.state == PullRequestState::Open)
                .or(first),
        };
        chosen.map(|pull| found(repository, pull)).transpose()
    }

    /// Open a pull request into `base` from `head` on `fork`'s fork of
    /// `repository`, sent to GitHub as `fork:head`.
    ///
    /// GitHub reads `fork:head` as `head` on the copy of `repository` that
    /// `fork` owns, so a `fork` that is `repository`'s own owner names
    /// `repository` itself: a fork that owner holds of its own repository,
    /// under another name, as an organisation may, cannot be named here.
    ///
    /// A `fork` owner GitHub does not allow is
    /// [`PullRequestError::InvalidRepositoryName`], and nothing is sent. One
    /// already open from that head is
    /// [`PullRequestError::PullRequestAlreadyExists`], naming `head` without
    /// its owner; [`Self::find_pull_request`] with the same `fork` finds it.
    pub async fn create_pull_request_from(
        &self,
        repository: &Repository,
        token: &SecretValue,
        fork: &str,
        head: &BranchName,
        base: &BranchName,
        title: &str,
        body: &str,
        draft: bool,
    ) -> PullRequestResult<CreatedPullRequest> {
        let owner = owned(fork)?;
        self.open_pull_request(
            repository,
            token,
            &format!("{owner}:{head}"),
            head,
            base,
            title,
            body,
            draft,
        )
        .await
    }

    /// Page `page` of `repository`'s pull requests in `state`, the most
    /// recently updated first, at `per_page` rows a page up to GitHub's
    /// hundred.
    ///
    /// Each row is read whole or not at all: one that does not say whether it
    /// is a draft, or that this crate cannot otherwise read, is
    /// [`PullRequestError::GitHubApi`], and one whose branch or head commit
    /// this crate does not accept is [`PullRequestError::Parse`], never a page
    /// without it. A row that names no head commit is listed with none.
    pub async fn list_pull_requests(
        &self,
        repository: &Repository,
        token: &SecretValue,
        state: StateFilter,
        page: NonZeroU32,
        per_page: NonZeroU8,
    ) -> PullRequestResult<Page<ListedPullRequest>> {
        let asked = asked(per_page);
        let mut url = self.repository_endpoint(repository, &["pulls"])?;
        url.query_pairs_mut()
            .append_pair("state", state.as_str())
            .append_pair("sort", "updated")
            .append_pair("direction", "desc");
        let rows: Vec<GitHubListedPull> = self.get(paged(url, page, asked), token).await?;
        let read = rows.len();

        let pulls = rows
            .into_iter()
            .map(|pull| listed(repository, pull))
            .collect::<PullRequestResult<Vec<ListedPullRequest>>>()?;
        Ok(Page::after(pulls, page, read, asked))
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
            &self.repository_endpoint(repository, &["pulls", &number, "files"])?,
            token,
        )
        .await
    }

    /// Page `page` of the files a pull request changes, in the order GitHub
    /// lists them, at `per_page` rows a page up to GitHub's hundred.
    ///
    /// Unlike [`Self::fetch_files`], which stops after ten pages, this reads
    /// as far as a caller follows [`Page::next`], up to the three thousand
    /// files GitHub lists for a pull request. A page longer than the 16 MiB
    /// this crate reads is [`PullRequestError::GitHubApi`], never a partial
    /// or empty page.
    pub async fn list_files(
        &self,
        reference: &PullRequestReference,
        token: &SecretValue,
        page: NonZeroU32,
        per_page: NonZeroU8,
    ) -> PullRequestResult<Page<ChangedFile>> {
        let asked = asked(per_page);
        let number = reference.number().to_string();
        let repository = reference.repository();
        let url = self.repository_endpoint(repository, &["pulls", &number, "files"])?;
        let files: Vec<ChangedFile> = self.get(paged(url, page, asked), token).await?;
        let read = files.len();
        Ok(Page::after(files, page, read, asked))
    }
}

/// `owner` when GitHub allows it to own a repository, or else
/// [`PullRequestError::InvalidRepositoryName`].
fn owned(owner: &str) -> PullRequestResult<&str> {
    if named(owner) {
        Ok(owner)
    } else {
        Err(PullRequestError::InvalidRepositoryName)
    }
}

/// Whether a listed pull request's head is `owner`'s branch `head`, by the
/// `owner:branch` label GitHub gives it; an owner is matched without regard
/// to case, as GitHub matches one, and a branch exactly.
fn headed(listed: &GitHubListedBranch, owner: &str, head: &BranchName) -> bool {
    listed
        .label
        .as_deref()
        .and_then(|label| label.split_once(':'))
        .is_some_and(|(labelled, branch)| {
            labelled.eq_ignore_ascii_case(owner) && branch == head.as_str()
        })
}

/// A pull request found in `repository`, its branches parsed; one that does
/// not say whether it is a draft is unreadable.
fn found(repository: &Repository, pull: GitHubListedPull) -> PullRequestResult<FoundPullRequest> {
    let draft = drafted(&pull)?;
    Ok(FoundPullRequest {
        reference: PullRequestReference::new(repository.clone(), pull.number),
        node_id: pull.node_id,
        title: pull.title,
        url: pull.html_url,
        head: BranchName::parse(&pull.head.reference)?,
        base: BranchName::parse(&pull.base.reference)?,
        state: pull.state,
        draft,
        merged: pull.merged_at.is_some_and(|at| !at.is_empty()),
    })
}

/// A pull request listed in `repository`, its branches and head commit
/// parsed; one that does not say whether it is a draft is unreadable.
fn listed(repository: &Repository, pull: GitHubListedPull) -> PullRequestResult<ListedPullRequest> {
    let draft = drafted(&pull)?;
    Ok(ListedPullRequest {
        reference: PullRequestReference::new(repository.clone(), pull.number),
        node_id: pull.node_id,
        title: pull.title,
        url: pull.html_url,
        state: pull.state,
        draft,
        head: BranchName::parse(&pull.head.reference)?,
        base: BranchName::parse(&pull.base.reference)?,
        head_sha: pull.head.sha.as_deref().map(CommitSha::parse).transpose()?,
        updated_at: pull.updated_at,
    })
}

/// Whether a listed pull request is a draft, which a row that does not say
/// leaves unreadable.
fn drafted(pull: &GitHubListedPull) -> PullRequestResult<bool> {
    pull.draft
        .ok_or_else(|| PullRequestError::GitHubApi(UNREADABLE.to_string()))
}

/// The pull request GitHub answered with, its branches and commits parsed.
fn detail(pull: GitHubPull) -> PullRequestResult<PullRequestDetail> {
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
        updated_at: pull.updated_at,
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
    use crate::pull_request::service::fixtures::project;
    use crate::pull_request::service::fixtures::query_is;
    use crate::pull_request::service::fixtures::seven;
    use crate::pull_request::service::fixtures::stand_in;
    use crate::pull_request::service::fixtures::token;
    use serde_json::Value;
    use serde_json::json;
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::body_json;
    use wiremock::matchers::header;
    use wiremock::matchers::method;
    use wiremock::matchers::path;
    use wiremock::matchers::query_param;

    const PULL: &str = "/repos/acme/project/pulls/7";

    const FILES: &str = "/repos/acme/project/pulls/7/files";

    const PULLS: &str = "/repos/acme/project/pulls";

    const UPDATED: &str = "2026-11-06T09:30:00Z";

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
            "updated_at": UPDATED,
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
                updated_at: Some(UPDATED.to_string()),
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
            "updated_at",
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
        assert_eq!(pull.updated_at, None);
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

    fn branch(name: &str) -> BranchName {
        BranchName::parse(name).unwrap()
    }

    fn page(number: u32) -> NonZeroU32 {
        NonZeroU32::new(number).unwrap()
    }

    fn rows(count: u8) -> NonZeroU8 {
        NonZeroU8::new(count).unwrap()
    }

    /// A pull request as GitHub lists it, numbered `number`, in `state`,
    /// merging the branch `label` names into `base`.
    fn row(number: u64, state: &str, label: &str, base: &str) -> Value {
        let (_, head) = label.split_once(':').unwrap();
        json!({
            "node_id": format!("PR_{number}"),
            "number": number,
            "title": format!("pull request {number}"),
            "html_url": format!("https://github.com/acme/project/pull/{number}"),
            "state": state,
            "draft": false,
            "updated_at": UPDATED,
            "head": { "ref": head, "label": label, "sha": commit('a').as_str() },
            "base": { "ref": base, "label": format!("acme:{base}") },
        })
    }

    /// `count` open pull requests numbered from `first`, each from its own
    /// branch into `main`.
    fn listing_of(first: u64, count: u64) -> Vec<Value> {
        (first..first + count)
            .map(|number| row(number, "open", &format!("acme:task/{number}"), "main"))
            .collect()
    }

    async fn listing(answer: Vec<Value>) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(PULLS))
            .respond_with(ResponseTemplate::new(200).set_body_json(answer))
            .mount(&server)
            .await;
        server
    }

    async fn find(
        server: &MockServer,
        fork: Option<&str>,
    ) -> PullRequestResult<Option<FoundPullRequest>> {
        let service = stand_in(server).await;
        service
            .find_pull_request(
                &project(&service),
                &token(),
                &branch("feature"),
                &branch("main"),
                fork,
            )
            .await
    }

    fn opened(head: &str) -> Value {
        json!({
            "id": 9,
            "number": 12,
            "html_url": "https://github.com/acme/project/pull/12",
            "state": "open",
            "title": "(feat): basket totals",
            "body": "Adds totals to the basket.",
            "head": { "ref": head, "sha": commit('a').as_str() },
            "base": { "ref": "main", "sha": commit('b').as_str() },
        })
    }

    /// The exact body a pull request from `head` into `main` is opened with.
    fn opening(head: &str) -> Value {
        json!({
            "title": "(feat): basket totals",
            "body": "Adds totals to the basket.",
            "head": head,
            "base": "main",
            "draft": false,
        })
    }

    async fn open(
        server: &MockServer,
        fork: Option<&str>,
    ) -> PullRequestResult<CreatedPullRequest> {
        let service = stand_in(server).await;
        let repository = project(&service);
        let head = branch("feature/one");
        let base = branch("main");
        let title = "(feat): basket totals";
        let body = "Adds totals to the basket.";
        match fork {
            Some(fork) => {
                service
                    .create_pull_request_from(
                        &repository,
                        &token(),
                        fork,
                        &head,
                        &base,
                        title,
                        body,
                        false,
                    )
                    .await
            }
            None => {
                service
                    .create_pull_request(&repository, &token(), &head, &base, title, body, false)
                    .await
            }
        }
    }

    async fn list(
        server: &MockServer,
        state: StateFilter,
        number: u32,
        per_page: u8,
    ) -> PullRequestResult<Page<ListedPullRequest>> {
        let service = stand_in(server).await;
        service
            .list_pull_requests(
                &project(&service),
                &token(),
                state,
                page(number),
                rows(per_page),
            )
            .await
    }

    fn numbers(pulls: &[ListedPullRequest]) -> Vec<u64> {
        pulls
            .iter()
            .map(|pull| pull.reference.number().get())
            .collect()
    }

    #[tokio::test]
    async fn an_open_pull_request_listed_after_a_newer_closed_one_is_chosen() {
        let server = listing(vec![
            row(9, "closed", "acme:feature", "main"),
            row(8, "open", "acme:feature", "main"),
            row(7, "closed", "acme:feature", "main"),
        ])
        .await;

        let found = find(&server, None).await.unwrap().unwrap();

        assert_eq!(found.reference.number().get(), 8);
        assert_eq!(found.state, PullRequestState::Open);
    }

    #[tokio::test]
    async fn the_newest_closed_pull_request_is_chosen_when_none_is_open() {
        let mut merged = row(9, "closed", "acme:feature", "main");
        merged["merged_at"] = json!(UPDATED);
        let server = listing(vec![merged, row(7, "closed", "acme:feature", "main")]).await;

        let found = find(&server, None).await.unwrap().unwrap();

        assert_eq!(found.reference.number().get(), 9);
        assert!(found.merged);
    }

    #[tokio::test]
    async fn a_found_pull_request_is_read_with_its_title() {
        let server = listing(vec![row(8, "open", "acme:feature", "main")]).await;
        let service = stand_in(&server).await;

        let found = find(&server, None).await.unwrap().unwrap();

        assert_eq!(
            found,
            FoundPullRequest::new(
                PullRequestReference::new(project(&service), NonZeroU64::new(8).unwrap()),
                "PR_8",
                "https://github.com/acme/project/pull/8",
                branch("feature"),
                branch("main"),
            )
            .with_title("pull request 8")
        );
    }

    #[tokio::test]
    async fn a_found_row_that_does_not_say_whether_it_is_a_draft_is_unreadable() {
        let mut undecided = row(8, "open", "acme:feature", "main");
        undecided.as_object_mut().unwrap().remove("draft");
        let server = listing(vec![undecided]).await;

        let failure = find(&server, None)
            .await
            .expect_err("a row with no draft state must not be read as ready");

        assert!(
            matches!(&failure, PullRequestError::GitHubApi(message) if message == UNREADABLE),
            "{failure:?}"
        );
    }

    #[tokio::test]
    async fn a_listed_row_without_a_title_is_unreadable() {
        let mut untitled = row(8, "open", "acme:feature", "main");
        untitled.as_object_mut().unwrap().remove("title");
        let server = listing(vec![untitled.clone()]).await;
        let paged = listing(vec![untitled]).await;

        let found = find(&server, None)
            .await
            .expect_err("a row with no title must not be read as untitled");
        let listed = list(&paged, StateFilter::Open, 1, 10)
            .await
            .expect_err("a row with no title must not be read as untitled");

        for failure in [found, listed] {
            assert!(
                matches!(&failure, PullRequestError::GitHubApi(message) if message == UNREADABLE),
                "{failure:?}"
            );
        }
    }

    #[tokio::test]
    async fn a_fork_s_label_names_its_owner_whatever_the_case() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(PULLS))
            .and(query_param("head", "fork-owner:feature"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([
                row(5, "open", "other-owner:feature", "main"),
                row(4, "open", "Fork-Owner:feature", "develop"),
                row(3, "open", "Fork-Owner:feature", "main"),
            ])))
            .expect(1)
            .mount(&server)
            .await;

        let found = find(&server, Some("fork-owner")).await.unwrap().unwrap();

        assert_eq!(
            found.reference.number().get(),
            3,
            "another owner's branch and another base are passed over"
        );
    }

    #[tokio::test]
    async fn the_lookup_asks_for_every_state_newest_first_and_reads_to_its_last_row() {
        let mut answer: Vec<Value> = (1..100)
            .map(|number| row(number, "open", "acme:other", "main"))
            .collect();
        answer.push(row(100, "open", "acme:feature", "main"));
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(PULLS))
            .and(query_is(&[
                ("head", "acme:feature"),
                ("base", "main"),
                ("state", "all"),
                ("sort", "created"),
                ("direction", "desc"),
                ("per_page", "100"),
            ]))
            .respond_with(ResponseTemplate::new(200).set_body_json(answer))
            .expect(1)
            .mount(&server)
            .await;

        let found = find(&server, None).await.unwrap().unwrap();

        assert_eq!(found.reference.number().get(), 100);
    }

    #[tokio::test]
    async fn no_matching_pull_request_is_none() {
        let server = listing(vec![
            row(2, "open", "acme:other", "main"),
            row(1, "open", "acme:feature", "develop"),
        ])
        .await;

        assert_eq!(find(&server, None).await.unwrap(), None);
    }

    #[tokio::test]
    async fn a_fork_owner_github_does_not_allow_is_refused_before_asking() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
            .expect(0)
            .mount(&server)
            .await;

        for owner in ["", "fork owner", "..", "owner/other"] {
            let failure = find(&server, Some(owner))
                .await
                .expect_err("an owner GitHub does not allow must not be asked for");

            assert!(
                matches!(failure, PullRequestError::InvalidRepositoryName),
                "{owner:?} gave {failure:?}"
            );
        }
    }

    #[tokio::test]
    async fn a_pull_request_from_a_fork_is_sent_with_its_owner_s_label() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(PULLS))
            .and(header("authorization", "Bearer token"))
            .and(body_json(opening("fork-owner:feature/one")))
            .respond_with(ResponseTemplate::new(201).set_body_json(opened("feature/one")))
            .expect(1)
            .mount(&server)
            .await;

        let created = open(&server, Some("fork-owner")).await.unwrap();

        assert_eq!(created.number.get(), 12);
        assert_eq!(created.state, PullRequestState::Open);
        assert_eq!(created.url, "https://github.com/acme/project/pull/12");
    }

    #[tokio::test]
    async fn a_pull_request_from_the_repository_itself_is_sent_its_bare_head() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(PULLS))
            .and(body_json(opening("feature/one")))
            .respond_with(ResponseTemplate::new(201).set_body_json(opened("feature/one")))
            .expect(1)
            .mount(&server)
            .await;

        let created = open(&server, None).await.unwrap();

        assert_eq!(created.number.get(), 12);
    }

    #[tokio::test]
    async fn a_pull_request_already_open_from_a_fork_s_head_is_reported_as_existing() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(PULLS))
            .and(body_json(opening("fork-owner:feature/one")))
            .respond_with(ResponseTemplate::new(422).set_body_json(json!({
                "message": "Validation Failed",
                "errors": [{
                    "resource": "PullRequest",
                    "code": "custom",
                    "message": "A pull request already exists for fork-owner:feature/one.",
                }],
            })))
            .expect(1)
            .mount(&server)
            .await;

        let failure = open(&server, Some("fork-owner")).await.unwrap_err();

        assert!(
            matches!(
                &failure,
                PullRequestError::PullRequestAlreadyExists(head) if *head == branch("feature/one")
            ),
            "{failure:?}"
        );
    }

    #[tokio::test]
    async fn a_fork_owner_github_does_not_allow_opens_nothing() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(201).set_body_json(opened("feature/one")))
            .expect(0)
            .mount(&server)
            .await;

        for owner in ["", "fork owner", "..", "owner/other", "owner:other"] {
            let failure = open(&server, Some(owner))
                .await
                .expect_err("an owner GitHub does not allow must not be sent");

            assert!(
                matches!(failure, PullRequestError::InvalidRepositoryName),
                "{owner:?} gave {failure:?}"
            );
        }
    }

    #[test]
    fn a_repository_is_built_on_the_host_github_answers_for_or_its_api_host() {
        let github = PullRequestService::new().unwrap();

        for host in ["github.com", "API.github.com", " github.com "] {
            let repository = github.repository_on(host, "acme", "project").expect(host);

            assert_eq!(repository, Repository::new("acme", "project").unwrap());
        }
        for host in [
            "gitlab.com",
            "github.example.com",
            "github.com.attacker.test",
            "",
        ] {
            assert!(
                matches!(
                    github.repository_on(host, "acme", "project"),
                    Err(PullRequestError::InvalidRepositoryUrl)
                ),
                "{host:?} was accepted for api.github.com"
            );
        }
        assert!(matches!(
            github.repository_on("github.com", "acme", "../project"),
            Err(PullRequestError::InvalidRepositoryName)
        ));
    }

    #[test]
    fn an_enterprise_install_builds_repositories_only_on_its_own_host() {
        let enterprise =
            PullRequestService::configured("https://github.example.com/api/v3").unwrap();

        assert_eq!(
            enterprise
                .repository_on("github.example.com", "acme", "project")
                .unwrap(),
            Repository::new("acme", "project").unwrap()
        );
        for host in ["github.com", "api.github.com"] {
            assert!(
                matches!(
                    enterprise.repository_on(host, "acme", "project"),
                    Err(PullRequestError::InvalidRepositoryUrl)
                ),
                "{host} was accepted for an Enterprise install"
            );
        }
    }

    #[tokio::test]
    async fn the_listing_asks_for_its_state_and_page_most_recently_updated_first() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(PULLS))
            .and(header("authorization", "Bearer token"))
            .and(query_is(&[
                ("state", "closed"),
                ("sort", "updated"),
                ("direction", "desc"),
                ("per_page", "10"),
                ("page", "2"),
            ]))
            .respond_with(ResponseTemplate::new(200).set_body_json(listing_of(11, 10)))
            .expect(1)
            .mount(&server)
            .await;

        let listed = list(&server, StateFilter::Closed, 2, 10).await.unwrap();

        assert_eq!(numbers(&listed.items), (11..21).collect::<Vec<u64>>());
        assert_eq!(listed.next, Some(page(3)), "a full page is followed");
    }

    #[tokio::test]
    async fn a_short_listing_page_is_the_last() {
        let server = listing(listing_of(11, 9)).await;

        let listed = list(&server, StateFilter::Open, 2, 10).await.unwrap();

        assert_eq!(listed.items.len(), 9);
        assert_eq!(listed.next, None);
    }

    #[tokio::test]
    async fn a_listing_page_never_asks_for_more_than_github_sends() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(PULLS))
            .and(query_is(&[
                ("state", "all"),
                ("sort", "updated"),
                ("direction", "desc"),
                ("per_page", "100"),
                ("page", "1"),
            ]))
            .respond_with(ResponseTemplate::new(200).set_body_json(listing_of(1, 100)))
            .expect(1)
            .mount(&server)
            .await;

        let listed = list(&server, StateFilter::All, 1, 200).await.unwrap();

        assert_eq!(listed.items.len(), 100);
        assert_eq!(listed.next, Some(page(2)), "a hundred of a hundred is full");
    }

    #[tokio::test]
    async fn a_listed_pull_request_is_read_whole() {
        let mut drafted = row(12, "closed", "acme:task/one", "main");
        drafted["draft"] = json!(true);
        let server = listing(vec![drafted]).await;
        let service = stand_in(&server).await;

        let listed = list(&server, StateFilter::All, 1, 10).await.unwrap();

        assert_eq!(
            listed.items,
            vec![
                ListedPullRequest::new(
                    PullRequestReference::new(project(&service), NonZeroU64::new(12).unwrap()),
                    "PR_12",
                    "https://github.com/acme/project/pull/12",
                    branch("task/one"),
                    branch("main"),
                )
                .with_title("pull request 12")
                .with_state(PullRequestState::Closed)
                .with_draft(true)
                .with_head_sha(commit('a'))
                .with_updated_at(UPDATED)
            ]
        );
    }

    #[tokio::test]
    async fn a_listed_row_that_does_not_say_whether_it_is_a_draft_fails_the_page() {
        let mut undecided = row(2, "open", "acme:task/two", "main");
        undecided.as_object_mut().unwrap().remove("draft");
        let server = listing(vec![row(1, "open", "acme:task/one", "main"), undecided]).await;

        let failure = list(&server, StateFilter::Open, 1, 10)
            .await
            .expect_err("a row with no draft state must not be read as ready, nor left out");

        assert!(
            matches!(&failure, PullRequestError::GitHubApi(message) if message == UNREADABLE),
            "{failure:?}"
        );
    }

    #[tokio::test]
    async fn a_listed_head_with_no_commit_has_none_and_a_malformed_one_is_refused() {
        let mut unpointed = row(1, "open", "acme:task/one", "main");
        unpointed["head"].as_object_mut().unwrap().remove("sha");
        let mut malformed = row(2, "open", "acme:task/two", "main");
        malformed["head"]["sha"] = json!("abc123");
        let quiet = listing(vec![unpointed]).await;
        let garbled = listing(vec![row(3, "open", "acme:task/three", "main"), malformed]).await;

        let listed = list(&quiet, StateFilter::Open, 1, 10).await.unwrap();
        let failure = list(&garbled, StateFilter::Open, 1, 10)
            .await
            .expect_err("a head commit this crate does not accept must not be dropped");

        assert_eq!(listed.items[0].head_sha, None);
        assert!(
            matches!(&failure, PullRequestError::Parse(ParseError::CommitSha(value)) if value == "abc123"),
            "{failure:?}"
        );
    }

    #[tokio::test]
    async fn changed_files_are_listed_a_page_at_a_time() {
        let file = |name: &str| json!({ "filename": name, "status": "modified", "additions": 1, "deletions": 1 });
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(FILES))
            .and(header("authorization", "Bearer token"))
            .and(query_is(&[("per_page", "2"), ("page", "1")]))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!([file("src/a.rs"), file("src/b.rs")])),
            )
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path(FILES))
            .and(query_is(&[("per_page", "2"), ("page", "2")]))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([file("src/c.rs")])))
            .expect(1)
            .mount(&server)
            .await;
        let service = stand_in(&server).await;
        let reference = seven(&service);

        let first = service
            .list_files(&reference, &token(), page(1), rows(2))
            .await
            .unwrap();
        let last = service
            .list_files(&reference, &token(), first.next.unwrap(), rows(2))
            .await
            .unwrap();

        assert_eq!(
            first,
            Page::new(
                vec![
                    changed("src/a.rs", FileStatus::Modified, 1),
                    changed("src/b.rs", FileStatus::Modified, 1),
                ],
                Some(page(2))
            )
        );
        assert_eq!(
            last,
            Page::new(vec![changed("src/c.rs", FileStatus::Modified, 1)], None)
        );
    }

    #[tokio::test]
    async fn a_files_page_never_asks_for_more_than_github_sends() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(FILES))
            .and(query_is(&[("per_page", "100"), ("page", "4")]))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
            .expect(1)
            .mount(&server)
            .await;
        let service = stand_in(&server).await;

        let files = service
            .list_files(&seven(&service), &token(), page(4), rows(u8::MAX))
            .await
            .unwrap();

        assert_eq!(files, Page::new(Vec::new(), None));
    }
}
