//! Pull requests on GitHub, and how each one was received.
//!
//! [`PrService`] opens a pull request when a task completes with code changes,
//! and reads back when it merged, how many rounds of review it took, who
//! approved it, and what the reviewers actually said. It addresses GitHub's own
//! API by default and a GitHub Enterprise origin when one is configured; either
//! way it only answers for repositories on the host that origin serves.

mod create_request;
mod created;
mod description;
mod error;
mod github_branch;
mod github_comment;
mod github_pull_request;
mod github_pull_request_detail;
mod github_review;
mod github_user;
mod mergeability;
mod origin;
mod reception;
mod reference;
mod review_state;
mod review_tally;
mod service;
mod submitted_review;

pub use crate::pull_request::created::CreatedPr;
pub use crate::pull_request::description::Description;
pub use crate::pull_request::error::PrError;
pub use crate::pull_request::error::PrResult;
pub use crate::pull_request::github_branch::GitHubBranch;
pub use crate::pull_request::github_pull_request::GitHubPullRequest;
pub use crate::pull_request::mergeability::Mergeability;
pub use crate::pull_request::reception::PullRequestReception;
pub use crate::pull_request::reception::minutes_between;
pub use crate::pull_request::reference::PullRequestReference;
pub use crate::pull_request::review_state::ReviewState;
pub use crate::pull_request::review_tally::ReviewTally;
pub use crate::pull_request::review_tally::tally;
pub use crate::pull_request::service::PrService;
pub use crate::pull_request::submitted_review::SubmittedReview;
