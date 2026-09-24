//! Pull requests on GitHub, and how each one was received.
//!
//! [`PullRequestService`] opens a pull request when a task completes with code changes,
//! and reads back when it merged, how many rounds of review it took, who
//! approved it, and what the reviewers actually said. It addresses GitHub's own
//! API by default and a GitHub Enterprise origin when one is configured; either
//! way it only answers for repositories on the host that origin serves.

mod changed_file;
mod checks_outcome;
mod create_request;
mod created;
mod created_repository;
mod description;
mod detail;
mod error;
mod excerpt;
mod file_status;
mod github_branch;
mod github_comment;
mod github_pull_request;
mod github_pull_request_detail;
mod github_refusal;
mod github_review;
mod github_review_thread;
mod github_review_threads;
mod github_thread_comment;
mod github_user;
mod graphql_connection;
mod graphql_error;
mod graphql_page_info;
mod graphql_pull_request;
mod graphql_repository;
mod graphql_request;
mod graphql_response;
mod issue_comment;
mod merge_method;
mod mergeability;
mod mergeable_state;
mod merged;
mod origin;
mod reception;
mod reference;
mod repository;
mod repository_detail;
mod repository_path;
mod review_event;
mod review_state;
mod review_tally;
mod review_thread_record;
mod service;
mod state;
mod submitted_review;
mod thread_comment;

pub use crate::pull_request::changed_file::ChangedFile;
pub use crate::pull_request::checks_outcome::ChecksOutcome;
pub use crate::pull_request::created::CreatedPullRequest;
pub use crate::pull_request::created_repository::CreatedRepository;
pub use crate::pull_request::description::Description;
pub use crate::pull_request::detail::PullRequestDetail;
pub use crate::pull_request::error::PullRequestError;
pub use crate::pull_request::error::PullRequestResult;
pub use crate::pull_request::excerpt::Excerpt;
pub use crate::pull_request::file_status::FileStatus;
pub use crate::pull_request::github_branch::GitHubBranch;
pub use crate::pull_request::github_pull_request::GitHubPullRequest;
pub use crate::pull_request::issue_comment::IssueComment;
pub use crate::pull_request::merge_method::MergeMethod;
pub use crate::pull_request::mergeability::Mergeability;
pub use crate::pull_request::mergeable_state::MergeableState;
pub use crate::pull_request::merged::MergedPullRequest;
pub use crate::pull_request::reception::PullRequestReception;
pub use crate::pull_request::reception::minutes_between;
pub use crate::pull_request::reference::PullRequestReference;
pub use crate::pull_request::repository::Repository;
pub use crate::pull_request::repository_path::RepositoryPath;
pub use crate::pull_request::review_event::ReviewEvent;
pub use crate::pull_request::review_state::ReviewState;
pub use crate::pull_request::review_tally::ReviewTally;
pub use crate::pull_request::review_tally::tally;
pub use crate::pull_request::review_thread_record::ReviewThreadRecord;
pub use crate::pull_request::service::PullRequestService;
pub use crate::pull_request::state::PullRequestState;
pub use crate::pull_request::submitted_review::SubmittedReview;
pub use crate::pull_request::thread_comment::ThreadComment;
