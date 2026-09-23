use crate::pull_request::CreatedPr;
use crate::pull_request::GitHubPullRequest;
use crate::pull_request::Mergeability;
use crate::pull_request::PrError;
use crate::pull_request::PrResult;
use crate::pull_request::PullRequestReception;
use crate::pull_request::PullRequestReference;
use crate::pull_request::ReviewState;
use crate::pull_request::SubmittedReview;
use crate::pull_request::create_request::CreatePrRequest;
use crate::pull_request::github_comment::GitHubComment;
use crate::pull_request::github_pull_request_detail::GitHubPullRequestDetail;
use crate::pull_request::github_review::GitHubReview;
use crate::pull_request::minutes_between;
use crate::pull_request::origin::Origin;
use crate::pull_request::origin::host_of;
use crate::pull_request::tally;
use reqwest::Client;
use serde::Deserialize;

/// What this crate calls itself to the GitHub API.
const USER_AGENT: &str = "abnegate-vcs";

/// GitHub's own REST origin, which answers for repositories on `github.com`.
const GITHUB_API_URL: &str = "https://api.github.com";

/// Rows GitHub returns per page; its maximum for these collections.
const PAGE_SIZE: usize = 100;

/// Pages a paged read will follow before it stops. A pull request with more
/// review activity than this has long since stopped teaching anything new, and
/// an unbounded walk would let one pathological change stall the sync.
const MAXIMUM_PAGES: usize = 10;

/// PR service for creating pull requests
#[derive(Debug, Clone)]
pub struct PrService {
    client: Client,
    origin: Origin,
}

impl Default for PrService {
    fn default() -> Self {
        Self::new()
    }
}

/// Schemes a repository address may carry.
///
/// The scp-like `git@host:owner/repo` has none and is read on its own terms.
const SCHEMES: [&str; 2] = ["https", "ssh"];

fn named(segment: &str) -> bool {
    !segment.is_empty()
        && segment != "."
        && segment != ".."
        && segment.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        })
}

impl PrService {
    /// Address GitHub's own API.
    pub fn new() -> Self {
        Self::configured(GITHUB_API_URL.to_string())
    }

    /// Address the origin an operator configured, for the repositories it
    /// answers for.
    pub fn configured(url: String) -> Self {
        Self {
            client: Client::new(),
            origin: Origin::configured(url),
        }
    }

    /// Address `url` as a stand-in for repositories on `host`.
    ///
    /// No operator setting produces one: a configured origin has to answer for
    /// a host it can be reached at. Publication tests use this to drive the
    /// real request path against a mock server.
    pub fn standing_in_for(host: &str, url: String) -> Self {
        Self {
            client: Client::new(),
            origin: Origin::standing_in_for(host, url),
        }
    }

    /// Parse owner and repo from a repository URL.
    ///
    /// The host has to be the one this service's origin answers for, so that a
    /// GitHub Enterprise install parses its own repositories and nothing else.
    /// Matching `https://github.com/` literally was what made `GITHUB_API_URL`
    /// insufficient on its own to reach Enterprise; matching nothing at all
    /// would accept a repository this service cannot open a request against;
    /// and admitting `github.com` alongside the configured origin sent a
    /// github.com repository's token to whichever install was configured.
    ///
    /// Supports, for the host it answers for:
    ///
    /// - `https://host/owner/repo`, with or without `.git`
    /// - `git@host:owner/repo.git`
    /// - `ssh://git@host/owner/repo.git`
    ///
    /// A scheme this does not speak is refused rather than discarded: reading
    /// the owner and repo out of an `ftp://` or `http://` address treats it as
    /// a repository this service publishes to, which is not what it is.
    pub fn parse_github_url(&self, url: &str) -> PrResult<(String, String)> {
        let invalid = || PrError::InvalidRepoUrl(url.to_string());
        let url = url.trim();

        // `git@host:owner/repo` is not a URL, so it is split on the colon
        // rather than parsed. The scp-like form has no scheme to strip.
        let (authority, path) = if let Some((scheme, rest)) = url.split_once("://") {
            if !SCHEMES
                .iter()
                .any(|allowed| scheme.eq_ignore_ascii_case(allowed))
            {
                return Err(invalid());
            }
            rest.split_once('/').ok_or_else(invalid)?
        } else if let Some((authority, path)) = url.split_once(':') {
            (authority, path)
        } else {
            return Err(invalid());
        };

        if !self.origin.answers_for(host_of(authority)) {
            return Err(invalid());
        }

        let path = path.trim_matches('/').trim_end_matches(".git");
        let mut segments = path.split('/').filter(|segment| !segment.is_empty());
        let owner = segments.next().ok_or_else(invalid)?;
        let repository = segments.next().ok_or_else(invalid)?;
        // The pair is interpolated into `{base}/repos/{owner}/{repo}/...`, so a
        // third segment or a traversal component would reach a different
        // endpoint than the caller asked for.
        if segments.next().is_some() || !named(owner) || !named(repository) {
            return Err(invalid());
        }

        Ok((owner.to_string(), repository.to_string()))
    }

    /// Recover where a pull request lives from the URL a run recorded.
    ///
    /// Reads `https://host/owner/repo/pull/123`, with or without a trailing
    /// segment such as `/files` that a person's copied link often carries. The
    /// host is held to the same origin as a repository URL, because the pull
    /// request is read back from `{origin}/repos/{owner}/{repo}/pulls/{number}`
    /// -- a recorded github.com link would otherwise be read from, and
    /// authenticated against, whichever install happened to be configured.
    pub fn pull_request(&self, url: &str) -> PrResult<PullRequestReference> {
        let invalid = || PrError::InvalidRepoUrl(url.to_string());
        let (scheme, rest) = url.trim().split_once("://").ok_or_else(invalid)?;
        let (authority, path) = rest.split_once('/').ok_or_else(invalid)?;

        if !matches!(scheme, "http" | "https") || !self.origin.answers_for(host_of(authority)) {
            return Err(invalid());
        }

        let mut segments = path.split('/');
        let owner = segments.next().unwrap_or_default();
        let repository = segments.next().unwrap_or_default().trim_end_matches(".git");
        let marker = segments.next().unwrap_or_default();
        let number: i64 = segments
            .next()
            .unwrap_or_default()
            .parse()
            .map_err(|_| invalid())?;

        if marker != "pull" || number < 1 || !named(owner) || !named(repository) {
            return Err(invalid());
        }

        Ok(PullRequestReference {
            owner: owner.to_string(),
            repository: repository.to_string(),
            number,
        })
    }

    /// Create a pull request on GitHub
    pub async fn create_pr(
        &self,
        owner: &str,
        repo: &str,
        token: &str,
        head_branch: &str,
        base_branch: &str,
        title: &str,
        body: &str,
        draft: bool,
    ) -> PrResult<CreatedPr> {
        let url = format!("{}/repos/{}/{}/pulls", self.origin.url, owner, repo);

        let request = CreatePrRequest {
            title: title.to_string(),
            body: body.to_string(),
            head: head_branch.to_string(),
            base: base_branch.to_string(),
            draft,
        };

        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {token}"))
            .header("Accept", "application/vnd.github+json")
            .header("User-Agent", USER_AGENT)
            .json(&request)
            .send()
            .await?;

        let status = response.status();

        if status.is_success() {
            let pr: GitHubPullRequest = response.json().await?;
            return Ok(CreatedPr {
                url: pr.html_url,
                number: pr.number,
                state: pr.state,
            });
        }

        // Handle error responses
        let error_text = response.text().await.unwrap_or_default();

        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(PrError::AuthFailed);
        }

        if status == reqwest::StatusCode::NOT_FOUND {
            return Err(PrError::BranchNotFound(head_branch.to_string()));
        }

        if status == reqwest::StatusCode::UNPROCESSABLE_ENTITY
            && error_text.contains("A pull request already exists")
        {
            return Err(PrError::PrAlreadyExists(head_branch.to_string()));
        }

        Err(PrError::GitHubApi(format!(
            "GitHub API returned {}: {}",
            status, error_text
        )))
    }

    /// Get the default branch for a repository
    pub async fn get_default_branch(
        &self,
        owner: &str,
        repo: &str,
        token: &str,
    ) -> PrResult<String> {
        let url = format!("{}/repos/{}/{}", self.origin.url, owner, repo);

        let response = self
            .client
            .get(&url)
            .header("Authorization", format!("Bearer {token}"))
            .header("Accept", "application/vnd.github+json")
            .header("User-Agent", USER_AGENT)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            return Err(PrError::GitHubApi(format!(
                "Failed to get repo info: {} {}",
                status, error_text
            )));
        }

        #[derive(Deserialize)]
        struct RepoInfo {
            default_branch: String,
        }

        let repo_info: RepoInfo = response.json().await?;
        Ok(repo_info.default_branch)
    }

    /// Check if a PR already exists for a branch
    pub async fn pr_exists_for_branch(
        &self,
        owner: &str,
        repo: &str,
        token: &str,
        head_branch: &str,
    ) -> PrResult<Option<String>> {
        let url = format!(
            "{}/repos/{}/{}/pulls?head={}:{}&state=open",
            self.origin.url, owner, repo, owner, head_branch
        );

        let response = self
            .client
            .get(&url)
            .header("Authorization", format!("Bearer {token}"))
            .header("Accept", "application/vnd.github+json")
            .header("User-Agent", USER_AGENT)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            return Err(PrError::GitHubApi(format!(
                "Failed to check PRs: {} {}",
                status, error_text
            )));
        }

        let prs: Vec<GitHubPullRequest> = response.json().await?;

        if let Some(pr) = prs.first() {
            Ok(Some(pr.html_url.clone()))
        } else {
            Ok(None)
        }
    }

    async fn get<T: serde::de::DeserializeOwned>(&self, url: &str, token: &str) -> PrResult<T> {
        let response = self
            .client
            .get(url)
            .header("Authorization", format!("Bearer {token}"))
            .header("Accept", "application/vnd.github+json")
            .header("User-Agent", USER_AGENT)
            .send()
            .await?;

        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(PrError::AuthFailed);
        }
        if !status.is_success() {
            let error_text = response.text().await.unwrap_or_default();
            return Err(PrError::GitHubApi(format!(
                "GitHub API returned {}: {}",
                status, error_text
            )));
        }

        Ok(response.json().await?)
    }

    async fn get_all<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        token: &str,
    ) -> PrResult<Vec<T>> {
        let mut collected: Vec<T> = Vec::new();

        for page in 1..=MAXIMUM_PAGES {
            let url = format!(
                "{}/{}?per_page={}&page={}",
                self.origin.url, path, PAGE_SIZE, page
            );
            let batch: Vec<T> = self.get(&url, token).await?;
            let complete = batch.len() < PAGE_SIZE;
            collected.extend(batch);
            if complete {
                break;
            }
        }

        Ok(collected)
    }

    /// Ask GitHub whether a pull request's branch still merges with its base.
    ///
    /// One read, so a caller can skip the expensive work of reproducing a merge
    /// for the overwhelming majority of branches that do not conflict.
    pub async fn fetch_mergeability(
        &self,
        reference: &PullRequestReference,
        token: &str,
    ) -> PrResult<Mergeability> {
        let detail: GitHubPullRequestDetail = self
            .get(
                &format!(
                    "{}/repos/{}/{}/pulls/{}",
                    self.origin.url, reference.owner, reference.repository, reference.number
                ),
                token,
            )
            .await?;

        Ok(Mergeability::from_flag(detail.mergeable))
    }

    /// Read back how a pull request was received.
    ///
    /// Three reads: the pull request itself for the opening and merge times, its
    /// reviews for the cycle and approval tallies, and its inline review comments.
    /// Review bodies count as comments too, because a reviewer who requests changes
    /// with a paragraph of reasoning is saying the same thing as one who writes it
    /// on a line.
    pub async fn fetch_reception(
        &self,
        reference: &PullRequestReference,
        token: &str,
    ) -> PrResult<PullRequestReception> {
        let scope = format!("repos/{}/{}", reference.owner, reference.repository);

        let detail: GitHubPullRequestDetail = self
            .get(
                &format!("{}/{}/pulls/{}", self.origin.url, scope, reference.number),
                token,
            )
            .await?;

        let reviews: Vec<GitHubReview> = self
            .get_all(
                &format!("{}/pulls/{}/reviews", scope, reference.number),
                token,
            )
            .await?;

        let inline: Vec<GitHubComment> = self
            .get_all(
                &format!("{}/pulls/{}/comments", scope, reference.number),
                token,
            )
            .await?;

        let submitted: Vec<SubmittedReview> = reviews
            .iter()
            .map(|review| SubmittedReview {
                state: ReviewState::parse(review.state.as_deref().unwrap_or_default()),
                reviewer: review.user.as_ref().and_then(|user| user.login.clone()),
            })
            .collect();
        let tallied = tally(&submitted);

        let comments: Vec<String> = reviews
            .iter()
            .filter_map(|review| review.body.as_deref())
            .chain(inline.iter().filter_map(|comment| comment.body.as_deref()))
            .map(|body| body.trim().to_string())
            .filter(|body| !body.is_empty())
            .collect();

        let opened_at = detail.created_at.filter(|value| !value.is_empty());
        let merged_at = detail.merged_at.filter(|value| !value.is_empty());
        let minutes_to_merge = match (&opened_at, &merged_at) {
            (Some(opened), Some(merged)) => minutes_between(opened, merged),
            _ => None,
        };

        Ok(PullRequestReception {
            opened_at,
            merged_at,
            minutes_to_merge,
            review_cycles: tallied.cycles,
            approvals: tallied.approvals,
            state: detail.state.filter(|value| !value.is_empty()),
            comments,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_github_https_url() {
        let service = PrService::new();
        let (owner, repo) = service
            .parse_github_url("https://github.com/acme/project")
            .unwrap();
        assert_eq!(owner, "acme");
        assert_eq!(repo, "project");
    }

    /// Matching `https://github.com/` meant an Enterprise repository was
    /// refused as invalid, so `GITHUB_API_URL` alone could not reach one.
    #[test]
    fn an_enterprise_repository_parses_like_a_github_one() {
        let service = PrService::configured("https://github.example.com/api/v3".to_string());
        for url in [
            "https://github.example.com/acme/project",
            "https://github.example.com/acme/project.git",
            "ssh://git@github.example.com/acme/project.git",
            "git@github.example.com:acme/project.git",
            "  https://github.example.com/acme/project/  ",
        ] {
            assert_eq!(
                service.parse_github_url(url).expect(url),
                ("acme".to_string(), "project".to_string()),
                "{url}"
            );
        }
    }

    /// A subdomain-isolated Enterprise install serves its API from `api.` on
    /// the host its repositories live on, exactly as api.github.com does for
    /// github.com.
    #[test]
    fn an_api_subdomain_answers_for_the_host_beneath_it() {
        for origin in [
            "https://api.github.example.com",
            "https://github.example.com/api/v3",
        ] {
            assert_eq!(
                PrService::configured(origin.to_string())
                    .parse_github_url("https://github.example.com/acme/project")
                    .expect(origin),
                ("acme".to_string(), "project".to_string()),
                "{origin}"
            );
        }
    }

    /// The repository's host picks the origin its owner and repository are
    /// interpolated into, and only the configured origin's own host has one. A
    /// github.com repository accepted here would have had its request -- and
    /// its access token -- sent to the Enterprise install instead.
    #[test]
    fn a_github_repository_has_no_origin_while_enterprise_is_configured() {
        let enterprise = PrService::configured("https://github.example.com/api/v3".to_string());
        for url in [
            "https://github.com/acme/project",
            "https://github.com/acme/project.git",
            "git@github.com:acme/project.git",
            "https://github.com/acme/project/pull/7",
        ] {
            assert!(
                enterprise.parse_github_url(url).is_err(),
                "{url} must not be addressed at an origin that does not answer for github.com"
            );
        }
        assert!(
            enterprise
                .pull_request("https://github.com/acme/project/pull/7")
                .is_err(),
            "a recorded github.com pull request must not be read from the Enterprise origin"
        );
    }

    /// The public path is what almost every deployment runs, so it has to stay
    /// exactly as it was: GitHub's own origin answers for github.com.
    #[test]
    fn githubs_own_origin_answers_for_github_repositories() {
        for service in [
            PrService::new(),
            PrService::configured(GITHUB_API_URL.to_string()),
        ] {
            assert_eq!(
                service
                    .parse_github_url("https://github.com/acme/project")
                    .expect("github.com is what api.github.com answers for"),
                ("acme".to_string(), "project".to_string())
            );
            assert_eq!(
                service
                    .pull_request("https://github.com/acme/project/pull/7")
                    .expect("a github.com pull request is read from api.github.com")
                    .number,
                7
            );
        }
    }

    /// The endpoint the pair is interpolated into belongs to one host, so a
    /// repository somewhere else is not this service's to address -- whatever
    /// its path happens to look like.
    #[test]
    fn a_repository_on_another_host_is_refused() {
        let service = PrService::new();
        for url in [
            "https://gitlab.com/acme/project",
            "https://bitbucket.org/acme/project.git",
            "git@gitlab.com:acme/project.git",
            "https://github.com.attacker.test/acme/project",
            "https://notgithub.com/acme/project",
            "https://github.example.com/acme/project",
        ] {
            assert!(
                service.parse_github_url(url).is_err(),
                "{url} was accepted for api.github.com"
            );
        }

        let enterprise = PrService::configured("https://github.example.com/api/v3".to_string());
        for url in [
            "https://gitlab.com/acme/project",
            "https://github.example.com.attacker.test/acme/project",
        ] {
            assert!(
                enterprise.parse_github_url(url).is_err(),
                "a configured enterprise origin does not admit {url}"
            );
        }
    }

    /// The pair is interpolated into `{base}/repos/{owner}/{repo}/pulls`, so
    /// anything that could reach a different endpoint has to be refused. The
    /// old parser split into two and kept every remaining slash in `repo`.
    #[test]
    fn a_path_that_could_reach_another_endpoint_is_refused() {
        let service = PrService::new();
        for url in [
            "https://github.com/acme/project/extra",
            "https://github.com/acme/../admin",
            "https://github.com/../acme",
            "https://github.com/acme",
            "https://github.com/",
            "https://github.com/acme/pro ject",
            "not-a-url",
            "",
        ] {
            assert!(
                service.parse_github_url(url).is_err(),
                "{url:?} must be refused"
            );
        }
    }

    #[test]
    fn test_parse_github_https_url_with_git() {
        let service = PrService::new();
        let (owner, repo) = service
            .parse_github_url("https://github.com/acme/project.git")
            .unwrap();
        assert_eq!(owner, "acme");
        assert_eq!(repo, "project");
    }

    #[test]
    fn test_parse_github_ssh_url() {
        let service = PrService::new();
        let (owner, repo) = service
            .parse_github_url("git@github.com:acme/project.git")
            .unwrap();
        assert_eq!(owner, "acme");
        assert_eq!(repo, "project");
    }

    #[test]
    fn test_parse_invalid_url() {
        let service = PrService::new();
        let result = service.parse_github_url("not-a-github-url");
        assert!(result.is_err());
    }

    #[test]
    fn a_pull_request_url_yields_its_owner_repository_and_number() {
        let reference = PrService::new()
            .pull_request("https://github.com/acme/project/pull/42")
            .expect("a plain pull request URL must parse");
        assert_eq!(reference.owner, "acme");
        assert_eq!(reference.repository, "project");
        assert_eq!(reference.number, 42);
    }

    #[test]
    fn a_pull_request_url_parses_past_a_trailing_tab_segment() {
        let reference = PrService::new()
            .pull_request("https://github.com/acme/project/pull/42/files")
            .unwrap();
        assert_eq!(reference.number, 42);
    }

    #[test]
    fn a_url_that_is_not_a_pull_request_is_refused() {
        let service = PrService::new();
        for url in [
            "https://github.com/acme/project",
            "https://github.com/acme/project/issues/42",
            "https://github.com/acme/project/pull/zero",
            "https://github.com/acme/project/pull/0",
            "https://github.com/../project/pull/42",
            "https://example.test/acme/project/pull/42",
            "git@github.com:acme/project/pull/42",
            "",
        ] {
            assert!(
                service.pull_request(url).is_err(),
                "{url} is not a pull request and must not parse as one"
            );
        }
    }

    #[tokio::test]
    async fn reception_reads_the_merge_time_cycles_approvals_and_comments() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/repos/acme/project/pulls/7"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "created_at": "2026-09-04T09:00:00Z",
                "merged_at": "2026-09-04T10:00:00Z",
                "state": "closed",
            })))
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path("/repos/acme/project/pulls/7/reviews"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                { "state": "CHANGES_REQUESTED", "body": "needs a regression test", "user": { "login": "ada" } },
                { "state": "APPROVED", "body": "", "user": { "login": "ada" } },
                { "state": "APPROVED", "body": null, "user": { "login": "grace" } },
            ])))
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path("/repos/acme/project/pulls/7/comments"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                { "body": "rename this variable" },
                { "body": "   " },
            ])))
            .mount(&server)
            .await;

        let service = PrService::standing_in_for("github.com", server.uri());
        let reception = service
            .fetch_reception(
                &PullRequestReference {
                    owner: "acme".to_string(),
                    repository: "project".to_string(),
                    number: 7,
                },
                "token",
            )
            .await
            .expect("a reachable pull request must yield its reception");

        assert_eq!(reception.opened_at.as_deref(), Some("2026-09-04T09:00:00Z"));
        assert_eq!(reception.merged_at.as_deref(), Some("2026-09-04T10:00:00Z"));
        assert_eq!(reception.minutes_to_merge, Some(60));
        assert_eq!(reception.review_cycles, 1);
        assert_eq!(reception.approvals, 2);
        assert_eq!(
            reception.comments,
            vec!["needs a regression test", "rename this variable"],
            "review bodies and inline comments both carry reviewer corrections"
        );
    }

    #[tokio::test]
    async fn an_unmerged_pull_request_reports_no_merge_time() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/repos/acme/project/pulls/7"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "created_at": "2026-09-04T09:00:00Z",
                "merged_at": null,
                "state": "open",
            })))
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path("/repos/acme/project/pulls/7/reviews"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path("/repos/acme/project/pulls/7/comments"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
            .mount(&server)
            .await;

        let reception = PrService::standing_in_for("github.com", server.uri())
            .fetch_reception(
                &PullRequestReference {
                    owner: "acme".to_string(),
                    repository: "project".to_string(),
                    number: 7,
                },
                "token",
            )
            .await
            .unwrap();

        assert_eq!(reception.merged_at, None);
        assert_eq!(reception.minutes_to_merge, None);
        assert_eq!(reception.state.as_deref(), Some("open"));
        assert!(reception.comments.is_empty());
    }

    #[tokio::test]
    async fn mergeability_distinguishes_conflicted_from_not_yet_computed() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        for (reported, expected) in [
            (serde_json::json!(true), Mergeability::Clean),
            (serde_json::json!(false), Mergeability::Conflicted),
            (serde_json::json!(null), Mergeability::Unknown),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/repos/acme/project/pulls/7"))
                .respond_with(
                    ResponseTemplate::new(200)
                        .set_body_json(serde_json::json!({ "mergeable": reported })),
                )
                .mount(&server)
                .await;

            let mergeability = PrService::standing_in_for("github.com", server.uri())
                .fetch_mergeability(
                    &PullRequestReference {
                        owner: "acme".to_string(),
                        repository: "project".to_string(),
                        number: 7,
                    },
                    "token",
                )
                .await
                .unwrap();

            assert_eq!(mergeability, expected, "GitHub reported {reported}");
        }

        assert!(Mergeability::Conflicted.conflicted());
        assert!(
            !Mergeability::Unknown.conflicted(),
            "a branch nobody has checked is not a branch known to conflict"
        );
    }

    #[tokio::test]
    async fn a_rejected_token_is_reported_as_an_authentication_failure() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/acme/project/pulls/7"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;

        let failure = PrService::standing_in_for("github.com", server.uri())
            .fetch_reception(
                &PullRequestReference {
                    owner: "acme".to_string(),
                    repository: "project".to_string(),
                    number: 7,
                },
                "token",
            )
            .await
            .expect_err("an unauthorised read must not look like an empty pull request");

        assert!(matches!(failure, PrError::AuthFailed));
    }
}
