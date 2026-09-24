use crate::branch_name::BranchName;
use crate::pull_request::CreatedPullRequest;
use crate::pull_request::GitHubPullRequest;
use crate::pull_request::Mergeability;
use crate::pull_request::PullRequestError;
use crate::pull_request::PullRequestReception;
use crate::pull_request::PullRequestReference;
use crate::pull_request::PullRequestResult;
use crate::pull_request::Repository;
use crate::pull_request::ReviewState;
use crate::pull_request::SubmittedReview;
use crate::pull_request::create_request::CreateRequest;
use crate::pull_request::github_comment::GitHubComment;
use crate::pull_request::github_pull_request_detail::GitHubPullRequestDetail;
use crate::pull_request::github_refusal::GitHubRefusal;
use crate::pull_request::github_review::GitHubReview;
use crate::pull_request::minutes_between;
use crate::pull_request::origin::Origin;
use crate::pull_request::origin::host_of;
use crate::pull_request::repository_detail::RepositoryDetail;
use crate::pull_request::tally;
use abnegate_secret::SecretValue;
use abnegate_secret::sanitize;
use reqwest::Client;
use reqwest::Method;
use reqwest::RequestBuilder;
use reqwest::Response;
use reqwest::StatusCode;
use reqwest::header;
use reqwest::header::HeaderMap;
use reqwest::redirect::Policy;
use serde::de::DeserializeOwned;
use std::num::NonZeroU64;
use std::time::Duration;
use url::Url;

mod branches;
mod checks;
mod contents;
mod conversation;
#[cfg(test)]
mod fixtures;
mod graphql;
mod merging;
mod pulls;
#[cfg(test)]
mod redirect_tests;
mod repositories;
mod threads;

/// What this crate calls itself to the GitHub API.
const USER_AGENT: &str = "abnegate-vcs";

/// GitHub's own REST origin, which answers for repositories on `github.com`.
const GITHUB_API_URL: &str = "https://api.github.com";

/// The media type GitHub's REST API answers in.
const ACCEPT: &str = "application/vnd.github+json";

/// Rows GitHub returns per page; its maximum for these collections.
const PAGE_SIZE: usize = 100;

/// Pages a paged read will follow before it stops. A pull request with more
/// review activity than this has long since stopped teaching anything new, and
/// an unbounded walk would let one pathological change stall the sync.
const MAXIMUM_PAGES: usize = 10;

/// Longest a request may take from sending to the last byte of its answer.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Longest a connection may take to open.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Redirects within the origin a request follows before it stops.
const MAXIMUM_REDIRECTS: usize = 10;

/// The header GitHub reports the requests left in the current window in.
const RATE_LIMIT_REMAINING: &str = "x-ratelimit-remaining";

/// The header GitHub's secondary rate limit says how long to wait in.
const RETRY_AFTER: &str = "retry-after";

/// What GitHub says of a spent rate limit on a 403 that carries no header
/// saying so.
const RATE_LIMIT: &str = "rate limit";

/// Most of GitHub's own words carried into [`PullRequestError::GitHubApi`].
const MAXIMUM_ERROR_BYTES: usize = 1024;

/// What joins GitHub's words when several are carried on one line.
const SEPARATOR: &str = "; ";

/// Characters that end a line without being control characters.
const LINE_BREAKS: [char; 2] = ['\u{2028}', '\u{2029}'];

/// Most of an error body read for GitHub's words about it.
const MAXIMUM_REFUSAL_BYTES: usize = 64 * 1024;

/// Most bytes of a successful answer read. A longer one is refused whole,
/// never parsed in part.
///
/// The bound counts bytes, not characters. A character takes up to four bytes
/// in UTF-8 and up to six as a JSON `\u` escape, so a page of a hundred
/// comments at GitHub's limit of 65,536 characters each can outgrow it, and
/// such a page is an error rather than a shorter list.
const MAXIMUM_ANSWER_BYTES: usize = 16 * 1024 * 1024;

/// What an answer this crate cannot read is reported as, in place of the
/// parser's own message, which quotes the answer.
const UNREADABLE: &str = "GitHub answered in a form this crate cannot read";

/// What an answer longer than [`MAXIMUM_ANSWER_BYTES`] is reported as.
const OVERSIZED: &str = "GitHub's answer was larger than this crate reads";

/// What the answer to a request that is not a read is reported as when a
/// redirect carried the request somewhere else.
const REDIRECTED: &str =
    "GitHub redirected a request that is not a read, so its answer is not to that request";

/// What GitHub says when a pull request for the branch is already open.
const ALREADY_EXISTS: &str = "A pull request already exists";

/// Schemes a repository address may carry.
///
/// The scp-like `git@host:owner/repo` has none and is read on its own terms.
const SCHEMES: [&str; 2] = ["https", "ssh"];

/// The only scheme a recorded pull request link may use.
const HTTPS: &str = "https";

/// The path segment between a repository and a pull request's number.
const PULL: &str = "pull";

/// The suffix git's own URLs carry on a repository name.
const GIT_SUFFIX: &str = ".git";

/// Opens, reads, checks, reviews and merges pull requests on GitHub or a
/// GitHub Enterprise install, reads back how each one was received, and
/// creates repositories there.
///
/// - Opening: [`Self::get_default_branch`],
///   [`Self::pull_request_exists_for_branch`] and
///   [`Self::create_pull_request`].
/// - Reading: [`Self::fetch_pull`], [`Self::fetch_files`],
///   [`Self::fetch_diff`], [`Self::fetch_file`], [`Self::fetch_mergeability`]
///   and [`Self::fetch_reception`].
/// - Checking: [`Self::fetch_checks`].
/// - Reviewing: [`Self::fetch_issue_comments`], [`Self::post_issue_comment`],
///   [`Self::submit_review`], [`Self::fetch_review_threads`],
///   [`Self::reply_to_review_comment`] and [`Self::resolve_review_thread`].
/// - Merging: [`Self::merge`], then [`Self::delete_branch`].
/// - Creating a repository: [`Self::create_repository`].
///
/// Review threads, resolving one, and an administrator's merge go to GitHub's
/// GraphQL API; everything else goes to its REST API, at the same origin. A
/// request follows a redirect only while it stays on that origin.
///
/// A status that says what it means on its own is reported as that: 401 as
/// [`PullRequestError::AuthenticationFailed`], 429 or a 403 that spent the
/// rate limit as [`PullRequestError::RateLimited`], any other 403 as
/// [`PullRequestError::Forbidden`], and 404 as [`PullRequestError::NotFound`],
/// except that [`Self::delete_branch`] reads a 404 as a branch already gone.
/// Outside a merge, the errors GitHub's GraphQL API reports map onto the same
/// variants where GitHub names their kind. Of a refusal's answer, an error
/// keeps only GitHub's own message, bounded: never the raw answer, and never
/// the token.
///
/// A paged read takes a hundred rows a page and follows at most ten pages.
/// Every JSON answer, each page of a paged read included, is read up to
/// 16 MiB and never parsed in part, so a longer page is
/// [`PullRequestError::GitHubApi`] rather than a shorter list.
/// [`Self::fetch_diff`] and [`Self::fetch_file`] read only as far as the limit
/// their caller passes.
///
/// [`Self::merge`] asks through REST first, and only a refusal by branch
/// protection is asked again through the administrator's GraphQL mutation.
/// Errors GitHub's GraphQL API reports in answer to that mutation become
/// [`PullRequestError::Protected`], apart from a spent rate limit, which stays
/// [`PullRequestError::RateLimited`]. A refusal of the mutation at the HTTP
/// level, such as [`PullRequestError::Forbidden`] or
/// [`PullRequestError::NotFound`], passes through unchanged.
#[derive(Debug, Clone)]
pub struct PullRequestService {
    client: Client,
    origin: Origin,
}

impl PullRequestService {
    /// Address GitHub's own API.
    pub fn new() -> PullRequestResult<Self> {
        Self::configured(GITHUB_API_URL)
    }

    /// Address the origin an operator configured, for the repositories it
    /// answers for: an HTTPS URL, with or without a trailing slash.
    pub fn configured(url: &str) -> PullRequestResult<Self> {
        Ok(Self {
            client: client(true, REQUEST_TIMEOUT)?,
            origin: Origin::configured(url)?,
        })
    }

    /// Address `url` as a stand-in for repositories on `host`, over whatever
    /// scheme it names.
    ///
    /// No operator setting produces one: a configured origin has to answer for
    /// a host it can be reached at over HTTPS. Tests use this to drive the
    /// real request path against a mock server.
    #[cfg(any(test, feature = "test-support"))]
    pub fn standing_in_for(host: &str, url: &str) -> PullRequestResult<Self> {
        Ok(Self {
            client: client(false, REQUEST_TIMEOUT)?,
            origin: Origin::standing_in_for(host, url)?,
        })
    }

    /// Parse the repository a URL names.
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
    pub fn parse_github_url(&self, url: &str) -> PullRequestResult<Repository> {
        let invalid = || PullRequestError::InvalidRepositoryUrl;
        let url = url.trim();

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

        let path = path.trim_matches('/');
        let path = path.strip_suffix(GIT_SUFFIX).unwrap_or(path);
        let mut segments = path.split('/').filter(|segment| !segment.is_empty());
        let owner = segments.next().ok_or_else(invalid)?;
        let name = segments.next().ok_or_else(invalid)?;
        if segments.next().is_some() {
            return Err(invalid());
        }

        Repository::new(owner, name).ok_or_else(invalid)
    }

    /// Recover where a pull request lives from the URL a run recorded.
    ///
    /// Reads `https://host/owner/repo/pull/123`, with or without a trailing
    /// segment such as `/files` that a person's copied link often carries. The
    /// host is held to the same origin as a repository URL, because the pull
    /// request is read back from `{origin}/repos/{owner}/{repo}/pulls/{number}`
    /// -- a recorded github.com link would otherwise be read from, and
    /// authenticated against, whichever install happened to be configured.
    pub fn pull_request(&self, url: &str) -> PullRequestResult<PullRequestReference> {
        let invalid = || PullRequestError::InvalidRepositoryUrl;
        let url = Url::parse(url.trim()).map_err(|_| invalid())?;
        if url.scheme() != HTTPS
            || !url.username().is_empty()
            || url.password().is_some()
            || !url
                .host_str()
                .is_some_and(|host| self.origin.answers_for(host))
        {
            return Err(invalid());
        }

        let mut segments = url.path_segments().ok_or_else(invalid)?;
        let owner = segments.next().unwrap_or_default();
        let name = segments.next().unwrap_or_default();
        let name = name.strip_suffix(GIT_SUFFIX).unwrap_or(name);
        let marker = segments.next().unwrap_or_default();
        let number: NonZeroU64 = segments
            .next()
            .unwrap_or_default()
            .parse()
            .map_err(|_| invalid())?;
        if marker != PULL {
            return Err(invalid());
        }

        Ok(PullRequestReference::new(
            Repository::new(owner, name).ok_or_else(invalid)?,
            number,
        ))
    }

    /// Open a pull request from `head` into `base`.
    pub async fn create_pull_request(
        &self,
        repository: &Repository,
        token: &SecretValue,
        head: &BranchName,
        base: &BranchName,
        title: &str,
        body: &str,
        draft: bool,
    ) -> PullRequestResult<CreatedPullRequest> {
        let url = self
            .origin
            .endpoint(&["repos", repository.owner(), repository.name(), "pulls"]);
        let request = CreateRequest {
            title,
            body,
            head: head.as_str(),
            base: base.as_str(),
            draft,
        };

        let response = sent(
            self.request(Method::POST, url, token, ACCEPT)
                .json(&request),
        )
        .await?;

        let status = response.status();
        if status.is_success() {
            let created: GitHubPullRequest = decode(response).await?;
            return Ok(CreatedPullRequest {
                url: created.html_url,
                number: created.number,
                state: created.state,
            });
        }
        let refusal = explained(response).await?;
        if status == StatusCode::UNPROCESSABLE_ENTITY && refusal.mentions(ALREADY_EXISTS) {
            return Err(PullRequestError::PullRequestAlreadyExists(head.clone()));
        }
        Err(unexpected(status, &refusal))
    }

    /// The repository's default branch.
    pub async fn get_default_branch(
        &self,
        repository: &Repository,
        token: &SecretValue,
    ) -> PullRequestResult<BranchName> {
        let detail: RepositoryDetail = self
            .get(
                self.origin
                    .endpoint(&["repos", repository.owner(), repository.name()]),
                token,
            )
            .await?;
        Ok(BranchName::parse(&detail.default_branch)?)
    }

    /// The URL of the open pull request from `head`, if there is one.
    pub async fn pull_request_exists_for_branch(
        &self,
        repository: &Repository,
        token: &SecretValue,
        head: &BranchName,
    ) -> PullRequestResult<Option<String>> {
        let mut url =
            self.origin
                .endpoint(&["repos", repository.owner(), repository.name(), "pulls"]);
        url.query_pairs_mut()
            .append_pair("head", &format!("{}:{head}", repository.owner()))
            .append_pair("state", "open");

        let open: Vec<GitHubPullRequest> = self.get(url, token).await?;
        Ok(open.into_iter().next().map(|found| found.html_url))
    }

    /// A request to `url` that carries `token` and asks for an answer in `accept`.
    fn request(
        &self,
        method: Method,
        url: Url,
        token: &SecretValue,
        accept: &'static str,
    ) -> RequestBuilder {
        self.client
            .request(method, url)
            .bearer_auth(token.expose())
            .header(header::ACCEPT, accept)
    }

    /// Send `request` and read its answer as `T`, or as the refusal it is.
    async fn exchange<T: DeserializeOwned>(&self, request: RequestBuilder) -> PullRequestResult<T> {
        decode(answered(request).await?).await
    }

    async fn get<T: DeserializeOwned>(
        &self,
        url: Url,
        token: &SecretValue,
    ) -> PullRequestResult<T> {
        self.exchange(self.request(Method::GET, url, token, ACCEPT))
            .await
    }

    async fn get_all<T: DeserializeOwned>(
        &self,
        segments: &[&str],
        token: &SecretValue,
    ) -> PullRequestResult<Vec<T>> {
        let mut collected: Vec<T> = Vec::new();

        for page in 1..=MAXIMUM_PAGES {
            let mut url = self.origin.endpoint(segments);
            url.query_pairs_mut()
                .append_pair("per_page", &PAGE_SIZE.to_string())
                .append_pair("page", &page.to_string());
            let batch: Vec<T> = self.get(url, token).await?;
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
        token: &SecretValue,
    ) -> PullRequestResult<Mergeability> {
        let number = reference.number().to_string();
        let repository = reference.repository();
        let detail: GitHubPullRequestDetail = self
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
        token: &SecretValue,
    ) -> PullRequestResult<PullRequestReception> {
        let number = reference.number().to_string();
        let repository = reference.repository();
        let pull = [
            "repos",
            repository.owner(),
            repository.name(),
            "pulls",
            &number,
        ];

        let detail: GitHubPullRequestDetail = self.get(self.origin.endpoint(&pull), token).await?;
        let reviews: Vec<GitHubReview> = self
            .get_all(&[&pull[..], &["reviews"]].concat(), token)
            .await?;
        let inline: Vec<GitHubComment> = self
            .get_all(&[&pull[..], &["comments"]].concat(), token)
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
            state: detail.state,
            comments,
        })
    }
}

/// A client that gives up on a request that stalls, that follows a redirect
/// only while it stays on the origin the request was sent to, and that refuses
/// anything but HTTPS unless it is standing in for a test's mock server.
///
/// A redirect it will not follow comes back as the answer itself.
fn client(https_only: bool, timeout: Duration) -> PullRequestResult<Client> {
    let redirect = Policy::custom(|attempt| {
        let previous = attempt.previous();
        let within = previous
            .first()
            .is_some_and(|sent| sent.origin() == attempt.url().origin());
        if within && previous.len() <= MAXIMUM_REDIRECTS {
            attempt.follow()
        } else {
            attempt.stop()
        }
    });

    Ok(Client::builder()
        .user_agent(USER_AGENT)
        .timeout(timeout)
        .connect_timeout(CONNECT_TIMEOUT)
        .https_only(https_only)
        .redirect(redirect)
        .build()?)
}

/// The answer to `request`, whatever its status, from where it was sent.
///
/// Following a redirect turns any request but a read into a GET of wherever
/// the redirect points, or sends it on somewhere it was not addressed, so the
/// answer to one that is not a GET, from an address other than the one it was
/// sent to, is refused whatever it says.
async fn sent(request: RequestBuilder) -> PullRequestResult<Response> {
    let (client, request) = request.build_split();
    let request = request?;
    let read = request.method() == Method::GET;
    let url = request.url().clone();
    let response = client.execute(request).await?;
    if !read && response.url() != &url {
        return Err(PullRequestError::GitHubApi(REDIRECTED.to_string()));
    }
    Ok(response)
}

/// The successful answer to `request`, from where it was sent, or the refusal
/// it is.
async fn answered(request: RequestBuilder) -> PullRequestResult<Response> {
    let response = sent(request).await?;
    match response.status().is_success() {
        true => Ok(response),
        false => Err(refusal(response).await),
    }
}

/// A successful answer read as `T`, from no more than
/// [`MAXIMUM_ANSWER_BYTES`] of it. An answer that is longer, or is not the JSON
/// expected, is reported in fixed words, because the parser's own message
/// quotes it.
async fn decode<T: DeserializeOwned>(response: Response) -> PullRequestResult<T> {
    decode_within(response, MAXIMUM_ANSWER_BYTES).await
}

/// A successful answer read as `T`, refused unread when it declares more than
/// `limit` bytes and refused whole when it sends more.
async fn decode_within<T: DeserializeOwned>(
    response: Response,
    limit: usize,
) -> PullRequestResult<T> {
    let oversized = || PullRequestError::GitHubApi(OVERSIZED.to_string());
    if response
        .content_length()
        .is_some_and(|length| u64::try_from(limit).is_ok_and(|limit| length > limit))
    {
        return Err(oversized());
    }

    let (body, more) = read_prefix(response, limit).await?;
    if more {
        return Err(oversized());
    }
    serde_json::from_slice(&body).map_err(|_| PullRequestError::GitHubApi(UNREADABLE.to_string()))
}

/// What a status means on its own: a token GitHub did not accept, one it
/// accepted but will not let do this, a rate limit, or a repository or pull
/// request the token cannot see.
fn classified(status: StatusCode, headers: &HeaderMap) -> Option<PullRequestError> {
    let exhausted = headers
        .get(RATE_LIMIT_REMAINING)
        .is_some_and(|remaining| remaining.as_bytes() == b"0")
        || headers.contains_key(RETRY_AFTER);
    match status {
        StatusCode::UNAUTHORIZED => Some(PullRequestError::AuthenticationFailed),
        StatusCode::TOO_MANY_REQUESTS => Some(PullRequestError::RateLimited),
        StatusCode::FORBIDDEN if exhausted => Some(PullRequestError::RateLimited),
        StatusCode::FORBIDDEN => Some(PullRequestError::Forbidden),
        StatusCode::NOT_FOUND => Some(PullRequestError::NotFound),
        _ => None,
    }
}

/// A refusal no status explains, named by its status and GitHub's own words.
fn unexpected(status: StatusCode, refusal: &GitHubRefusal) -> PullRequestError {
    let summary = refusal.summary();
    let returned = returned(status);
    PullRequestError::GitHubApi(match summary.is_empty() {
        true => returned,
        false => format!("{returned}: {summary}"),
    })
}

/// An answer named by its status alone.
fn returned(status: StatusCode) -> String {
    format!("GitHub API returned {status}")
}

/// The first `limit` bytes of an answer's body, read no further, and whether
/// any of it remained unread.
async fn read_prefix(mut response: Response, limit: usize) -> PullRequestResult<(Vec<u8>, bool)> {
    let declared = response
        .content_length()
        .map_or(0, |length| usize::try_from(length).unwrap_or(usize::MAX));
    let mut prefix = Vec::with_capacity(declared.min(limit));
    while let Some(chunk) = response.chunk().await? {
        let room = limit - prefix.len();
        if chunk.len() > room {
            prefix.extend_from_slice(&chunk[..room]);
            return Ok((prefix, true));
        }
        prefix.extend_from_slice(&chunk);
    }
    Ok((prefix, false))
}

/// What GitHub said in refusing, read from no more than
/// [`MAXIMUM_REFUSAL_BYTES`] of its answer. A body that cannot be read says
/// nothing.
async fn refusal_of(response: Response) -> GitHubRefusal {
    read_prefix(response, MAXIMUM_REFUSAL_BYTES)
        .await
        .map(|(body, _)| GitHubRefusal::parse(&body))
        .unwrap_or_default()
}

/// What GitHub said in refusing, for a caller to read more into, unless the
/// refusal means something on its own, which is then the error.
///
/// A 403 that carries no rate-limit header is read before it is called
/// forbidden, because GitHub's secondary rate limit can say so only in words.
async fn explained(response: Response) -> PullRequestResult<GitHubRefusal> {
    match classified(response.status(), response.headers()) {
        Some(PullRequestError::Forbidden) => Err(forbidden(&refusal_of(response).await)),
        Some(failure) => Err(failure),
        None => Ok(refusal_of(response).await),
    }
}

/// What a 403 without a rate-limit header means: a spent rate limit when
/// GitHub's words say so, and otherwise a token that may not do this.
fn forbidden(refusal: &GitHubRefusal) -> PullRequestError {
    match refusal.mentions(RATE_LIMIT) {
        true => PullRequestError::RateLimited,
        false => PullRequestError::Forbidden,
    }
}

/// What an unsuccessful answer means: what it says on its own, or else its
/// status and GitHub's own words.
async fn refusal(response: Response) -> PullRequestError {
    let status = response.status();
    match explained(response).await {
        Ok(refusal) => unexpected(status, &refusal),
        Err(failure) => failure,
    }
}

/// `text` cut to [`MAXIMUM_ERROR_BYTES`], never inside a character.
pub(super) fn bounded(text: &str) -> &str {
    &text[..text.floor_char_boundary(MAXIMUM_ERROR_BYTES)]
}

/// GitHub's words on one line fit to carry in an error: each with any
/// credential redacted, and terminal sequences, invisible formatting, control
/// characters and line breaks removed, then trimmed, left out when nothing is
/// left, joined, and cut to [`MAXIMUM_ERROR_BYTES`].
pub(super) fn summarised<'a>(said: impl IntoIterator<Item = &'a str>) -> String {
    let mut line = String::new();
    for words in said {
        if line.len() >= MAXIMUM_ERROR_BYTES {
            break;
        }
        let cleaned = cleaned(words);
        if cleaned.is_empty() {
            continue;
        }
        if !line.is_empty() {
            line.push_str(SEPARATOR);
        }
        line.push_str(&cleaned);
    }
    bounded(&line).to_string()
}

/// `words` sanitised, without control characters or line breaks, and
/// trimmed.
fn cleaned(words: &str) -> String {
    let kept: String = sanitize(words)
        .chars()
        .filter(|character| !character.is_control() && !LINE_BREAKS.contains(character))
        .collect();
    kept.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pull_request::PullRequestState;
    use crate::pull_request::ReviewTally;
    use crate::pull_request::service::fixtures::Expected;
    use crate::pull_request::service::fixtures::commit;
    use crate::pull_request::service::fixtures::project;
    use crate::pull_request::service::fixtures::seven;
    use crate::pull_request::service::fixtures::stand_in;
    use crate::pull_request::service::fixtures::token;
    use tokio::io::AsyncReadExt;
    use tokio::io::AsyncWriteExt;
    use tokio::net::TcpListener;
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::body_partial_json;
    use wiremock::matchers::header;
    use wiremock::matchers::method;
    use wiremock::matchers::path;
    use wiremock::matchers::query_param;

    fn github() -> PullRequestService {
        PullRequestService::new().unwrap()
    }

    fn enterprise() -> PullRequestService {
        PullRequestService::configured("https://github.example.com/api/v3").unwrap()
    }

    fn pair(repository: Repository) -> (String, String) {
        (
            repository.owner().to_string(),
            repository.name().to_string(),
        )
    }

    fn acme() -> (String, String) {
        ("acme".to_string(), "project".to_string())
    }

    /// An answer of exactly `length` bytes that reads as a mergeable pull
    /// request, padded with a sentinel no error may repeat.
    fn padded(length: usize) -> String {
        let opening = r#"{"mergeable":true,"padding":"RAW-SENTINEL"#;
        let closing = r#""}"#;
        let padding = "x".repeat(length - opening.len() - closing.len());
        format!("{opening}{padding}{closing}")
    }

    /// `body` behind the length it declares.
    fn declared(body: &str) -> String {
        format!(
            "HTTP/1.1 200 OK\r\ncontent-length: {}\r\n\r\n{body}",
            body.len()
        )
    }

    /// `body` streamed in a chunk, with no length declared ahead of it.
    fn streamed(body: &str) -> String {
        format!(
            "HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\n{:x}\r\n{body}\r\n0\r\n\r\n",
            body.len()
        )
    }

    /// The response to a request sent to a server that answers once with
    /// `answer`, byte for byte.
    ///
    /// wiremock frames every body itself, so it can send neither a chunked
    /// body nor one shorter than the length it declares; these tests write
    /// the answer by hand instead.
    async fn served(answer: String) -> Response {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a loopback port to listen on");
        let address = listener
            .local_addr()
            .expect("the address the listener is bound to");
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("the client to connect");
            let mut request = Vec::new();
            let mut buffer = [0; 1024];
            while !request.ends_with(b"\r\n\r\n") {
                let read = stream
                    .read(&mut buffer)
                    .await
                    .expect("the request to be readable");
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..read]);
            }
            stream
                .write_all(answer.as_bytes())
                .await
                .expect("the answer to be writable");
        });

        client(false, REQUEST_TIMEOUT)
            .expect("a client")
            .get(format!("http://{address}"))
            .send()
            .await
            .expect("an answer from the stand-in server")
    }

    /// Following a 301, 302 or 303 turns any other method into a GET and drops
    /// its body; a 307 or 308 re-sends it somewhere it was not addressed.
    /// Either way the answer is not to the request that was sent.
    #[tokio::test]
    async fn an_answer_a_request_that_is_not_a_read_was_redirected_to_is_refused() {
        let server = MockServer::start().await;
        for (target, status) in [("found", 200), ("missing", 404)] {
            Mock::given(path(format!("/{target}")))
                .respond_with(ResponseTemplate::new(status).set_body_json(serde_json::json!({})))
                .mount(&server)
                .await;
        }
        for redirect in [301, 302, 303, 307, 308] {
            for target in ["found", "missing"] {
                Mock::given(path(format!("/{redirect}/{target}")))
                    .respond_with(
                        ResponseTemplate::new(redirect)
                            .insert_header("location", format!("{}/{target}", server.uri())),
                    )
                    .mount(&server)
                    .await;
            }
        }
        let client = client(false, REQUEST_TIMEOUT).unwrap();

        for redirect in [301, 302, 303, 307, 308] {
            for target in ["found", "missing"] {
                let url = format!("{}/{redirect}/{target}", server.uri());
                for changing in [Method::POST, Method::PUT, Method::DELETE] {
                    let answer = answered(
                        client
                            .request(changing.clone(), &url)
                            .json(&serde_json::json!({ "body": "sent once" })),
                    )
                    .await;

                    assert!(
                        matches!(answer, Err(PullRequestError::GitHubApi(ref text)) if text == REDIRECTED),
                        "{changing} {redirect} to {target}: {answer:?}"
                    );
                }
            }

            let read = answered(client.get(format!("{}/{redirect}/found", server.uri()))).await;
            assert!(read.is_ok(), "GET {redirect}: {read:?}");
        }

        for changing in [Method::POST, Method::PUT, Method::DELETE] {
            let answer =
                answered(client.request(changing.clone(), format!("{}/found", server.uri()))).await;
            assert!(answer.is_ok(), "{changing} unredirected: {answer:?}");
        }
    }

    #[test]
    fn a_github_repository_url_in_every_form_names_its_owner_and_repository() {
        for url in [
            "https://github.com/acme/project",
            "https://github.com/acme/project.git",
            "git@github.com:acme/project.git",
            "ssh://git@github.com/acme/project.git",
        ] {
            assert_eq!(
                pair(github().parse_github_url(url).expect(url)),
                acme(),
                "{url}"
            );
        }
        assert_eq!(
            github()
                .parse_github_url("https://github.com/acme/project")
                .unwrap()
                .to_string(),
            "acme/project"
        );
    }

    /// Matching `https://github.com/` meant an Enterprise repository was
    /// refused as invalid, so `GITHUB_API_URL` alone could not reach one.
    #[test]
    fn an_enterprise_repository_parses_like_a_github_one() {
        for url in [
            "https://github.example.com/acme/project",
            "https://github.example.com/acme/project.git",
            "ssh://git@github.example.com/acme/project.git",
            "git@github.example.com:acme/project.git",
            "  https://github.example.com/acme/project/  ",
        ] {
            assert_eq!(
                pair(enterprise().parse_github_url(url).expect(url)),
                acme(),
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
            "https://github.example.com/api/v3/",
        ] {
            assert_eq!(
                pair(
                    PullRequestService::configured(origin)
                        .unwrap()
                        .parse_github_url("https://github.example.com/acme/project")
                        .expect(origin)
                ),
                acme(),
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
        for url in [
            "https://github.com/acme/project",
            "https://github.com/acme/project.git",
            "git@github.com:acme/project.git",
            "https://github.com/acme/project/pull/7",
        ] {
            assert!(
                enterprise().parse_github_url(url).is_err(),
                "{url} must not be addressed at an origin that does not answer for github.com"
            );
        }
        assert!(
            enterprise()
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
            github(),
            PullRequestService::configured(GITHUB_API_URL).unwrap(),
        ] {
            assert_eq!(
                pair(
                    service
                        .parse_github_url("https://github.com/acme/project")
                        .expect("github.com is what api.github.com answers for")
                ),
                acme()
            );
            assert_eq!(seven(&service).number().get(), 7);
        }
    }

    /// The endpoint the pair is interpolated into belongs to one host, so a
    /// repository somewhere else is not this service's to address -- whatever
    /// its path happens to look like.
    #[test]
    fn a_repository_on_another_host_is_refused() {
        for url in [
            "https://gitlab.com/acme/project",
            "https://bitbucket.org/acme/project.git",
            "git@gitlab.com:acme/project.git",
            "https://github.com.attacker.test/acme/project",
            "https://notgithub.com/acme/project",
            "https://github.example.com/acme/project",
        ] {
            assert!(
                github().parse_github_url(url).is_err(),
                "{url} was accepted for api.github.com"
            );
        }
        for url in [
            "https://gitlab.com/acme/project",
            "https://github.example.com.attacker.test/acme/project",
        ] {
            assert!(
                enterprise().parse_github_url(url).is_err(),
                "a configured enterprise origin does not admit {url}"
            );
        }
    }

    /// The pair is interpolated into `{base}/repos/{owner}/{repo}/pulls`, so
    /// anything that could reach a different endpoint has to be refused.
    #[test]
    fn a_path_that_could_reach_another_endpoint_is_refused() {
        for url in [
            "https://github.com/acme/project/extra",
            "https://github.com/acme/../admin",
            "https://github.com/../acme",
            "https://github.com/acme",
            "https://github.com/",
            "https://github.com/acme/pro ject",
            "https://github.com/acme/.git",
            "not-a-github-url",
            "",
        ] {
            assert!(
                github().parse_github_url(url).is_err(),
                "{url:?} must be refused"
            );
        }
    }

    #[test]
    fn a_pull_request_url_yields_its_repository_and_number() {
        let reference = github()
            .pull_request("https://github.com/acme/project/pull/42")
            .expect("a plain pull request URL must parse");
        assert_eq!(pair(reference.repository().clone()), acme());
        assert_eq!(reference.number().get(), 42);

        let trailing = github()
            .pull_request("https://github.com/acme/project/pull/42/files")
            .unwrap();
        assert_eq!(trailing, reference);
    }

    #[test]
    fn a_url_that_is_not_a_pull_request_is_refused() {
        for url in [
            "https://github.com/acme/project",
            "https://github.com/acme/project/issues/42",
            "https://github.com/acme/project/pull/zero",
            "https://github.com/acme/project/pull/0",
            "https://github.com/acme/project/pull/-1",
            "https://github.com/../project/pull/42",
            "http://github.com/acme/project/pull/42",
            "https://token@github.com/acme/project/pull/42",
            "https://example.test/acme/project/pull/42",
            "git@github.com:acme/project/pull/42",
            "",
        ] {
            assert!(
                github().pull_request(url).is_err(),
                "{url} is not a pull request and must not parse as one"
            );
        }
    }

    #[test]
    fn an_origin_that_is_not_https_is_refused() {
        for origin in [
            "http://api.github.com",
            "api.github.com",
            "https://api.github.com/?x=1",
        ] {
            assert!(
                matches!(
                    PullRequestService::configured(origin),
                    Err(PullRequestError::InvalidOrigin)
                ),
                "{origin}"
            );
        }
    }

    #[test]
    fn a_pull_request_state_github_has_not_named_yet_is_unknown() {
        for (state, expected) in [
            ("\"open\"", PullRequestState::Open),
            ("\"closed\"", PullRequestState::Closed),
            ("\"merged-somehow\"", PullRequestState::Unknown),
        ] {
            assert_eq!(
                serde_json::from_str::<PullRequestState>(state).unwrap(),
                expected
            );
        }
    }

    #[test]
    fn a_pull_request_numbered_zero_is_not_one() {
        let pull = |number: u64| {
            serde_json::json!({
                "id": 1,
                "number": number,
                "html_url": "https://github.com/acme/project/pull/1",
                "state": "open",
                "title": "title",
                "body": null,
                "head": { "ref": "feature", "sha": commit('a').as_str() },
                "base": { "ref": "main", "sha": commit('b').as_str() },
            })
        };

        assert!(serde_json::from_value::<GitHubPullRequest>(pull(0)).is_err());
        let parsed: GitHubPullRequest = serde_json::from_value(pull(1)).unwrap();
        assert_eq!(parsed.head.reference, "feature");
    }

    #[tokio::test]
    async fn a_pull_request_is_opened_on_the_repository_it_was_parsed_from() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/repos/acme/project/pulls"))
            .and(header("authorization", "Bearer token"))
            .and(body_partial_json(serde_json::json!({
                "head": "feature/one",
                "base": "main",
                "draft": true,
            })))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
                "id": 9,
                "number": 12,
                "html_url": "https://github.com/acme/project/pull/12",
                "state": "open",
                "title": "(fix): title",
                "body": "body",
                "head": { "ref": "feature/one", "sha": commit('a').as_str() },
                "base": { "ref": "main", "sha": commit('b').as_str() },
            })))
            .mount(&server)
            .await;
        let service = stand_in(&server).await;
        let repository = project(&service);

        let created = service
            .create_pull_request(
                &repository,
                &token(),
                &BranchName::parse("feature/one").unwrap(),
                &BranchName::parse("main").unwrap(),
                "(fix): title",
                "body",
                true,
            )
            .await
            .unwrap();

        assert_eq!(created.number.get(), 12);
        assert_eq!(created.state, PullRequestState::Open);
        assert_eq!(created.url, "https://github.com/acme/project/pull/12");
    }

    #[tokio::test]
    async fn an_open_pull_request_is_found_by_a_head_branch_sent_as_one_query_value() {
        let head = "fix/a+b#1&state=closed";
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/acme/project/pulls"))
            .and(query_param("head", format!("acme:{head}")))
            .and(query_param("state", "open"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!([{
                    "id": 9,
                    "number": 12,
                    "html_url": "https://github.com/acme/project/pull/12",
                    "state": "open",
                    "title": "title",
                    "body": null,
                    "head": { "ref": head, "sha": commit('a').as_str() },
                    "base": { "ref": "main", "sha": commit('b').as_str() },
                }])),
            )
            .mount(&server)
            .await;
        let service = stand_in(&server).await;
        let repository = project(&service);

        let found = service
            .pull_request_exists_for_branch(
                &repository,
                &token(),
                &BranchName::parse(head).unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(
            found.as_deref(),
            Some("https://github.com/acme/project/pull/12")
        );
    }

    #[tokio::test]
    async fn the_default_branch_is_read_as_a_branch_name() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/acme/project"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({ "default_branch": "trunk" })),
            )
            .mount(&server)
            .await;
        let service = stand_in(&server).await;
        let repository = project(&service);

        assert_eq!(
            service
                .get_default_branch(&repository, &token())
                .await
                .unwrap()
                .as_str(),
            "trunk"
        );
    }

    /// A 403 is a token GitHub accepted but will not let do this, or a spent
    /// rate limit; a 404 is something the token cannot see. Neither is a
    /// token GitHub refused, and a 404 on opening is not a missing branch.
    #[tokio::test]
    async fn every_refusal_is_reported_as_what_it_is() {
        let refusals: [(ResponseTemplate, Expected); 8] = [
            (ResponseTemplate::new(401), |failure| {
                matches!(failure, PullRequestError::AuthenticationFailed)
            }),
            (ResponseTemplate::new(403), |failure| {
                matches!(failure, PullRequestError::Forbidden)
            }),
            (
                ResponseTemplate::new(403).insert_header("x-ratelimit-remaining", "0"),
                |failure| matches!(failure, PullRequestError::RateLimited),
            ),
            (ResponseTemplate::new(429), |failure| {
                matches!(failure, PullRequestError::RateLimited)
            }),
            (
                ResponseTemplate::new(403).insert_header("retry-after", "60"),
                |failure| matches!(failure, PullRequestError::RateLimited),
            ),
            (ResponseTemplate::new(404), |failure| {
                matches!(failure, PullRequestError::NotFound)
            }),
            (
                ResponseTemplate::new(422).set_body_string(
                    "{\"message\":\"A pull request already exists for acme:feature.\"}",
                ),
                |failure| matches!(failure, PullRequestError::PullRequestAlreadyExists(_)),
            ),
            (ResponseTemplate::new(500), |failure| {
                matches!(failure, PullRequestError::GitHubApi(_))
            }),
        ];
        for (response, expected) in refusals {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/repos/acme/project/pulls"))
                .respond_with(response)
                .mount(&server)
                .await;
            let service = stand_in(&server).await;
            let repository = project(&service);

            let failure = service
                .create_pull_request(
                    &repository,
                    &token(),
                    &BranchName::parse("feature").unwrap(),
                    &BranchName::parse("main").unwrap(),
                    "title",
                    "body",
                    false,
                )
                .await
                .unwrap_err();

            assert!(expected(&failure), "{failure:?}");
        }
    }

    /// GitHub's secondary rate limit can answer with a bare 403 that says so
    /// only in words.
    #[tokio::test]
    async fn a_forbidden_answer_that_says_it_is_a_rate_limit_is_one() {
        for (message, expected) in [
            (
                "You have exceeded a secondary rate limit. Please wait a few minutes before you try again.",
                "RateLimited",
            ),
            (
                "API rate limit exceeded for installation ID 1.",
                "RateLimited",
            ),
            ("Resource not accessible by integration", "Forbidden"),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/repos/acme/project/pulls/7"))
                .respond_with(
                    ResponseTemplate::new(403)
                        .set_body_json(serde_json::json!({ "message": message })),
                )
                .expect(1)
                .mount(&server)
                .await;
            let service = stand_in(&server).await;

            let failure = service
                .fetch_mergeability(&seven(&service), &token())
                .await
                .unwrap_err();

            assert_eq!(format!("{failure:?}"), expected, "{message}");
        }
    }

    #[tokio::test]
    async fn an_error_body_is_carried_only_so_far() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(500)
                    .set_body_json(serde_json::json!({ "message": "é".repeat(4096) })),
            )
            .mount(&server)
            .await;
        let service = stand_in(&server).await;

        let failure = service
            .fetch_mergeability(&seven(&service), &token())
            .await
            .unwrap_err()
            .to_string();

        assert!(
            failure.len() < MAXIMUM_ERROR_BYTES + 100,
            "{}",
            failure.len()
        );
    }

    #[tokio::test]
    async fn a_prefix_stops_reading_at_its_limit_and_says_more_remained() {
        let limit = 100;
        for (length, expected, remained) in [
            (1_000_000, limit, true),
            (limit, limit, false),
            (0, 0, false),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .respond_with(ResponseTemplate::new(200).set_body_string("b".repeat(length)))
                .mount(&server)
                .await;
            let response = client(false, REQUEST_TIMEOUT)
                .unwrap()
                .get(server.uri())
                .send()
                .await
                .unwrap();

            let (prefix, more) = read_prefix(response, limit).await.unwrap();

            assert_eq!(prefix, "b".repeat(expected).into_bytes(), "{length} bytes");
            assert_eq!(more, remained, "{length} bytes");
        }
    }

    /// A chunk that fills the prefix exactly does not end the read; the one
    /// after it is what says more remained.
    #[tokio::test]
    async fn a_prefix_filled_on_a_chunk_boundary_still_says_more_remained() {
        let limit = 100;
        let filling = "a".repeat(limit);
        let answer = format!(
            "HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\n\
             {limit:x}\r\n{filling}\r\n1\r\nb\r\n0\r\n\r\n"
        );

        let (prefix, more) = read_prefix(served(answer).await, limit).await.unwrap();

        assert_eq!(prefix, filling.into_bytes());
        assert!(more, "a byte followed the chunk that filled the prefix");
    }

    /// However an answer is framed, it is read to its last byte while it fits,
    /// and one byte more is refused whole rather than parsed in part.
    #[tokio::test]
    async fn an_answer_is_read_up_to_its_bound_and_refused_past_it() {
        let limit = 100;
        for frame in [declared, streamed] {
            let within: GitHubPullRequestDetail =
                decode_within(served(frame(&padded(limit))).await, limit)
                    .await
                    .unwrap();
            assert_eq!(within.mergeable, Some(true));

            let failure = decode_within::<GitHubPullRequestDetail>(
                served(frame(&padded(limit + 1))).await,
                limit,
            )
            .await
            .unwrap_err();
            assert!(
                matches!(failure, PullRequestError::GitHubApi(ref text) if text == OVERSIZED),
                "{failure:?}"
            );
            assert!(
                !format!("{failure:?}").contains("RAW-SENTINEL"),
                "{failure:?}"
            );
        }
    }

    /// A length declared past the bound is refused on the headers alone, so a
    /// server that promises a huge answer is never read from at all.
    #[tokio::test]
    async fn a_length_declared_past_the_bound_is_refused_before_the_body_is_read() {
        let response =
            served("HTTP/1.1 200 OK\r\ncontent-length: 1000000\r\n\r\n{}".to_string()).await;

        let failure = decode_within::<serde_json::Value>(response, 100)
            .await
            .unwrap_err();

        assert!(
            matches!(failure, PullRequestError::GitHubApi(ref text) if text == OVERSIZED),
            "{failure:?}"
        );
    }

    /// An answer that stops short of the length it declared is a failed read,
    /// not one too long or one this crate cannot parse.
    #[tokio::test]
    async fn an_answer_cut_short_is_a_failed_read() {
        let response = served("HTTP/1.1 200 OK\r\ncontent-length: 100\r\n\r\n{}".to_string()).await;

        let failure = decode_within::<serde_json::Value>(response, 1024)
            .await
            .unwrap_err();

        assert!(matches!(failure, PullRequestError::Http(_)), "{failure:?}");
    }

    /// The service's own reads decode an answer exactly as long as the bound
    /// and refuse one a byte longer.
    #[tokio::test]
    async fn a_read_decodes_an_answer_as_long_as_the_bound_and_refuses_one_longer() {
        for (length, expected) in [
            (MAXIMUM_ANSWER_BYTES, Ok(Mergeability::Clean)),
            (
                MAXIMUM_ANSWER_BYTES + 1,
                Err(format!("GitHubApi({OVERSIZED:?})")),
            ),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/repos/acme/project/pulls/7"))
                .respond_with(ResponseTemplate::new(200).set_body_string(padded(length)))
                .mount(&server)
                .await;
            let service = stand_in(&server).await;

            let read = service
                .fetch_mergeability(&seven(&service), &token())
                .await
                .map_err(|failure| format!("{failure:?}"));

            assert_eq!(read, expected, "{length} bytes");
        }
    }

    /// GitHub's message is what explains a refusal; the rest of its answer is
    /// whatever the server chose to send, and has no place in an error a log
    /// keeps.
    #[tokio::test]
    async fn an_error_names_github_s_message_and_never_its_raw_body() {
        for (body, expected) in [
            (
                r#"{"message":"boom","documentation_url":"RAW-SENTINEL"}"#,
                "GitHub API returned 500 Internal Server Error: boom",
            ),
            (
                "<html>RAW-SENTINEL</html>",
                "GitHub API returned 500 Internal Server Error",
            ),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .respond_with(ResponseTemplate::new(500).set_body_string(body))
                .mount(&server)
                .await;
            let service = stand_in(&server).await;

            let failure = service
                .fetch_mergeability(&seven(&service), &token())
                .await
                .unwrap_err();

            assert!(
                matches!(failure, PullRequestError::GitHubApi(ref text) if text == expected),
                "{failure:?}"
            );
            assert_eq!(failure.to_string(), format!("GitHub API error: {expected}"));
            assert!(
                !format!("{failure:?}").contains("RAW-SENTINEL"),
                "{failure:?}"
            );
        }

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(422).set_body_json(serde_json::json!({
                "message": "Validation Failed",
                "errors": [{ "code": "custom", "message": "No commits between main and feature" }],
                "documentation_url": "RAW-SENTINEL",
            })))
            .mount(&server)
            .await;
        let service = stand_in(&server).await;

        let failure = service
            .create_pull_request(
                &project(&service),
                &token(),
                &BranchName::parse("feature").unwrap(),
                &BranchName::parse("main").unwrap(),
                "title",
                "body",
                false,
            )
            .await
            .unwrap_err();

        assert_eq!(
            failure.to_string(),
            "GitHub API error: GitHub API returned 422 Unprocessable Entity: \
             Validation Failed; No commits between main and feature"
        );
        assert!(
            !format!("{failure:?}").contains("RAW-SENTINEL"),
            "{failure:?}"
        );
    }

    /// The parser's own message quotes the value it could not read, which is
    /// the answer's content and not this crate's to repeat.
    #[tokio::test]
    async fn an_answer_this_crate_cannot_read_names_no_part_of_it() {
        for body in [r#"{"mergeable":"RAW-SENTINEL"}"#, "RAW-SENTINEL"] {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .respond_with(ResponseTemplate::new(200).set_body_string(body))
                .mount(&server)
                .await;
            let service = stand_in(&server).await;

            let failure = service
                .fetch_mergeability(&seven(&service), &token())
                .await
                .unwrap_err();

            assert!(
                matches!(failure, PullRequestError::GitHubApi(ref text) if text == UNREADABLE),
                "{failure:?}"
            );
            assert!(
                !format!("{failure:?}").contains("RAW-SENTINEL"),
                "{failure:?}"
            );
        }

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(201)
                    .set_body_json(serde_json::json!({ "number": "RAW-SENTINEL" })),
            )
            .mount(&server)
            .await;
        let service = stand_in(&server).await;

        let failure = service
            .create_pull_request(
                &project(&service),
                &token(),
                &BranchName::parse("feature").unwrap(),
                &BranchName::parse("main").unwrap(),
                "title",
                "body",
                false,
            )
            .await
            .unwrap_err();

        assert!(
            matches!(failure, PullRequestError::GitHubApi(ref text) if text == UNREADABLE),
            "{failure:?}"
        );
        assert!(
            !format!("{failure:?}").contains("RAW-SENTINEL"),
            "{failure:?}"
        );
    }

    /// A trailing slash on the configured origin used to open an empty path
    /// segment, so every request went to `//repos/...`.
    #[tokio::test]
    async fn an_origin_with_a_trailing_slash_addresses_the_same_endpoints() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v3/repos/acme/project/pulls/7"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({ "mergeable": true })),
            )
            .mount(&server)
            .await;
        let service =
            PullRequestService::standing_in_for("github.com", &format!("{}/api/v3/", server.uri()))
                .unwrap();

        assert_eq!(
            service
                .fetch_mergeability(&seven(&service), &token())
                .await
                .unwrap(),
            Mergeability::Clean
        );
    }

    /// A server that accepts the connection and never answers is given up on
    /// rather than waited for, by the same client every service is built with.
    #[tokio::test]
    async fn a_request_that_stalls_is_given_up_on() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({ "mergeable": true }))
                    .set_delay(Duration::from_secs(10)),
            )
            .mount(&server)
            .await;
        let service = PullRequestService {
            client: client(false, Duration::from_millis(200)).unwrap(),
            origin: Origin::standing_in_for("github.com", &server.uri()).unwrap(),
        };
        let started = std::time::Instant::now();

        let failure = service
            .fetch_mergeability(&seven(&service), &token())
            .await
            .unwrap_err();

        assert!(
            matches!(failure, PullRequestError::Http(ref error) if error.is_timeout()),
            "{failure:?}"
        );
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[tokio::test]
    async fn reception_reads_the_merge_time_cycles_approvals_and_comments() {
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
            .and(query_param("per_page", "100"))
            .and(query_param("page", "1"))
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

        let service = stand_in(&server).await;
        let reception = service
            .fetch_reception(&seven(&service), &token())
            .await
            .expect("a reachable pull request must yield its reception");

        assert_eq!(reception.opened_at.as_deref(), Some("2026-09-04T09:00:00Z"));
        assert_eq!(reception.merged_at.as_deref(), Some("2026-09-04T10:00:00Z"));
        assert_eq!(reception.minutes_to_merge, Some(60));
        assert_eq!(reception.review_cycles, 1);
        assert_eq!(reception.approvals, 2);
        assert_eq!(reception.state, Some(PullRequestState::Closed));
        assert_eq!(
            reception.comments,
            vec!["needs a regression test", "rename this variable"],
            "review bodies and inline comments both carry reviewer corrections"
        );
    }

    #[tokio::test]
    async fn an_unmerged_pull_request_reports_no_merge_time() {
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

        for collection in ["reviews", "comments"] {
            Mock::given(method("GET"))
                .and(path(format!("/repos/acme/project/pulls/7/{collection}")))
                .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([])))
                .mount(&server)
                .await;
        }

        let service = stand_in(&server).await;
        let reception = service
            .fetch_reception(&seven(&service), &token())
            .await
            .unwrap();

        assert_eq!(reception.merged_at, None);
        assert_eq!(reception.minutes_to_merge, None);
        assert_eq!(reception.state, Some(PullRequestState::Open));
        assert!(reception.comments.is_empty());
        assert_eq!(
            tally(&[]),
            ReviewTally::default(),
            "no reviews tally to nothing"
        );
    }

    #[tokio::test]
    async fn mergeability_distinguishes_conflicted_from_not_yet_computed() {
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

            let service = stand_in(&server).await;
            let mergeability = service
                .fetch_mergeability(&seven(&service), &token())
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
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/repos/acme/project/pulls/7"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;

        let service = stand_in(&server).await;
        let failure = service
            .fetch_reception(&seven(&service), &token())
            .await
            .expect_err("an unauthorised read must not look like an empty pull request");

        assert!(matches!(failure, PullRequestError::AuthenticationFailed));
    }
}
