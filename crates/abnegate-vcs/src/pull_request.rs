//! Pull requests on GitHub, from opening one to merging it, and the
//! repositories they land in.
//!
//! Only the `github` feature builds this module. [`PullRequestService`] opens a
//! pull request when a task completes with code changes; reads it back with
//! its files, its diff and the checks on its head; takes part in its review
//! through conversation comments, reviews and review threads; moves it in and
//! out of draft; sets it to merge once it is ready; merges it; and reads back
//! when it merged, how many rounds of review it took, who approved it, and
//! what the reviewers actually said. It also creates a repository for a new
//! project and reads a file from one, and registers, re-signs and removes the
//! webhooks a repository delivers its events through. It talks to GitHub's
//! REST API, and to its GraphQL API for review threads, an administrator's
//! merge, auto-merge, drafts and a repository's merge settings. It addresses
//! GitHub's own API by default and a GitHub Enterprise origin when one is
//! configured; either way it only answers for repositories on the host that
//! origin serves.
//!
//! Every URL a call reaches is built from numbers and validated values: a
//! [`Repository`] from [`PullRequestService::parse_github_url`] or
//! [`PullRequestService::repository_on`], a [`PullRequestReference`] from
//! [`PullRequestService::pull_request`], an [`IssueReference`], a
//! [`crate::BranchName`], a [`crate::CommitSha`], a [`crate::ObjectId`], a
//! [`RepositoryPath`], and a new repository's owner held to a
//! [`Repository`]'s rules. Each travels percent-encoded under the configured
//! origin, as path segments or as a query value, and a segment that is
//! empty, `.` or `..` is refused before anything is sent.
//!
//! Five values a caller passes as plain text are held to rules before they
//! travel:
//!
//! - the ref [`PullRequestService::resolve_commit`] resolves, held to a
//!   [`crate::BranchName`]'s rules unless it is `HEAD`, travels as one path
//!   segment, a `/` in it sent as `%2F`;
//! - the ref [`PullRequestService::fetch_tree_at`] reads a tree at, held to
//!   the same rules, travels as one path segment the same way;
//! - the ref [`PullRequestService::fetch_contents`] reads a file at, held to
//!   the same rules, travels as the `ref` query value;
//! - the tag [`PullRequestService::fetch_release`] reads, held to the same
//!   rules, travels as one path segment the same way;
//! - the fork owner [`PullRequestService::find_pull_request`] and
//!   [`PullRequestService::create_pull_request_from`] take, held to a
//!   [`Repository`] owner's rules, travels with the head branch as the
//!   `owner:branch` of the `head` query value or JSON field.
//!
//! Of the rest, the owner and head branch
//! [`PullRequestService::pull_request_exists_for_branch`] looks for travel
//! together as `head`, and the commit [`PullRequestService::fetch_file`]
//! reads at travels as `ref`. So no value, not even a path someone else
//! chose, reaches another endpoint or adds a parameter. Everything else a
//! call sends, such as a GraphQL node id, a title or a body, travels in its
//! JSON, never in its URL. What GitHub answers is held to the same types: a
//! branch or commit this crate does not accept is
//! [`PullRequestError::Parse`], never read as something else. A branch
//! GitHub lists is named by a [`crate::RemoteBranchName`], which keeps names
//! such as `+release` that a [`crate::BranchName`] refuses, so one such
//! branch never fails a page; it reaches a git command line only through
//! [`crate::RemoteBranchName::to_branch_name`].
//!
//! [`PullRequestService::fetch_diff`] and [`PullRequestService::fetch_file`]
//! read no more of the answer than the byte limit their caller passes, and
//! return an [`Excerpt`] that is never longer than that limit, never ends inside
//! a character, and says in [`Excerpt::truncated`] whether the text went on;
//! displayed, a cut excerpt ends with [`Excerpt::MARKER`].
//! [`PullRequestService::fetch_contents`] reads a file through GitHub's
//! contents API instead, no more of the answer than any JSON answer, and
//! returns its bytes decoded from base64 in a [`Contents`].
//!
//! [`WebhookSignature`] checks that a webhook delivery was signed with the
//! secret its webhook was registered with, before anything reads it.
//!
//! The `github-app` feature adds what authenticating as a GitHub App
//! installation takes, rather than with a personal access token:
//! `ApplicationClaims` signs the JWT that proves the caller is the App,
//! `InstallationIssuer` trades it for an installation access token, and
//! `TokenCache` holds each installation's token until it comes within five
//! minutes of expiring. `ApplicationService` registers an App from its
//! manifest and lists its installations and the repositories they reach.

#[cfg(feature = "github-app")]
mod application;
mod auto_merge;
mod automation;
mod blob;
mod branch;
mod changed_file;
mod check_run;
mod checks_outcome;
mod collected;
mod comment_request;
mod commit_status;
mod contents;
mod create_request;
mod created;
mod created_repository;
mod deployment;
mod deployment_status;
mod description;
mod detail;
mod diff_side;
mod enabled_auto_merge;
mod error;
mod excerpt;
mod file_status;
mod found;
mod github_branch;
mod github_check_conclusion;
mod github_check_run;
mod github_check_run_record;
mod github_check_runs;
mod github_check_status;
mod github_combined_status;
mod github_comment;
mod github_commit;
mod github_commit_status;
mod github_commit_status_record;
mod github_contents;
mod github_created_repository;
mod github_deployment;
mod github_deployment_status;
mod github_identified;
mod github_issue;
mod github_issue_comment;
mod github_issue_pull_request;
mod github_issue_request;
mod github_issue_row;
mod github_label;
mod github_listed_branch;
mod github_listed_pull;
mod github_merge;
mod github_pull;
mod github_pull_request;
mod github_pull_request_detail;
mod github_refusal;
mod github_release;
mod github_repository_branch;
mod github_repository_name;
mod github_review;
mod github_review_comment;
mod github_status_state;
mod github_tree;
mod github_tree_entry;
mod github_user;
mod github_workflow_job;
mod github_workflow_run;
mod github_workflow_runs;
mod graphql_actor;
mod graphql_auto_merge_request;
mod graphql_automated_repository;
mod graphql_automation;
mod graphql_connection;
mod graphql_error;
mod graphql_identified;
mod graphql_merge_settings;
mod graphql_page_info;
mod graphql_pull_request;
mod graphql_query;
mod graphql_repository;
mod graphql_request;
mod graphql_response;
mod graphql_review_thread;
mod graphql_thread_comment;
mod graphql_thread_state;
mod issue;
mod issue_comment;
mod issue_reference;
mod issue_state;
mod issue_update;
mod job_log;
mod job_log_error;
mod label;
mod listed;
mod merge_method;
mod merge_request;
mod merge_settings;
mod mergeability;
mod mergeable_state;
mod merged;
mod origin;
mod page;
mod rate_limit;
mod reception;
mod reference;
mod release;
mod repository;
mod repository_detail;
mod repository_path;
mod repository_request;
mod review_comment;
mod review_event;
mod review_record;
mod review_request;
mod review_state;
mod review_tally;
mod review_thread_count;
mod review_thread_record;
mod service;
mod state;
mod state_filter;
mod submitted_review;
mod thread_comment;
mod tree;
mod tree_entry;
mod tree_entry_kind;
mod webhook_configuration;
mod webhook_event;
mod webhook_request;
mod webhook_signature;
mod workflow_job;
mod workflow_run;
mod workflow_run_filter;

#[cfg(feature = "github-app")]
pub use crate::pull_request::application::Account;
#[cfg(feature = "github-app")]
pub use crate::pull_request::application::AccountType;
#[cfg(feature = "github-app")]
pub use crate::pull_request::application::ApplicationClaims;
#[cfg(feature = "github-app")]
pub use crate::pull_request::application::ApplicationError;
#[cfg(feature = "github-app")]
pub use crate::pull_request::application::ApplicationId;
#[cfg(feature = "github-app")]
pub use crate::pull_request::application::ApplicationService;
#[cfg(feature = "github-app")]
pub use crate::pull_request::application::Installation;
#[cfg(feature = "github-app")]
pub use crate::pull_request::application::InstallationId;
#[cfg(feature = "github-app")]
pub use crate::pull_request::application::InstallationIssuer;
#[cfg(feature = "github-app")]
pub use crate::pull_request::application::InstallationToken;
#[cfg(feature = "github-app")]
pub use crate::pull_request::application::InstalledRepository;
#[cfg(feature = "github-app")]
pub use crate::pull_request::application::RegisteredApplication;
#[cfg(feature = "github-app")]
pub use crate::pull_request::application::RepositorySelection;
#[cfg(feature = "github-app")]
pub use crate::pull_request::application::TokenCache;
pub use crate::pull_request::auto_merge::AutoMerge;
pub use crate::pull_request::automation::PullRequestAutomation;
pub use crate::pull_request::blob::Blob;
pub use crate::pull_request::branch::Branch;
pub use crate::pull_request::changed_file::ChangedFile;
pub use crate::pull_request::check_run::CheckRun;
pub use crate::pull_request::checks_outcome::ChecksOutcome;
pub use crate::pull_request::collected::Collected;
pub use crate::pull_request::commit_status::CommitStatus;
pub use crate::pull_request::contents::Contents;
pub use crate::pull_request::created::CreatedPullRequest;
pub use crate::pull_request::created_repository::CreatedRepository;
pub use crate::pull_request::deployment::Deployment;
pub use crate::pull_request::deployment_status::DeploymentStatus;
pub use crate::pull_request::description::Description;
pub use crate::pull_request::detail::PullRequestDetail;
pub use crate::pull_request::diff_side::DiffSide;
pub use crate::pull_request::enabled_auto_merge::EnabledAutoMerge;
pub use crate::pull_request::error::PullRequestError;
pub use crate::pull_request::error::PullRequestResult;
pub use crate::pull_request::excerpt::Excerpt;
pub use crate::pull_request::file_status::FileStatus;
pub use crate::pull_request::found::FoundPullRequest;
pub use crate::pull_request::github_branch::GitHubBranch;
pub use crate::pull_request::github_pull_request::GitHubPullRequest;
pub use crate::pull_request::issue::Issue;
pub use crate::pull_request::issue_comment::IssueComment;
pub use crate::pull_request::issue_reference::IssueReference;
pub use crate::pull_request::issue_state::IssueState;
pub use crate::pull_request::issue_update::IssueUpdate;
pub use crate::pull_request::job_log::JobLog;
pub use crate::pull_request::job_log_error::JobLogError;
pub use crate::pull_request::label::Label;
pub use crate::pull_request::listed::ListedPullRequest;
pub use crate::pull_request::merge_method::MergeMethod;
pub use crate::pull_request::merge_settings::MergeSettings;
pub use crate::pull_request::mergeability::Mergeability;
pub use crate::pull_request::mergeable_state::MergeableState;
pub use crate::pull_request::merged::MergedPullRequest;
pub use crate::pull_request::page::Page;
pub use crate::pull_request::rate_limit::RateLimit;
pub use crate::pull_request::reception::PullRequestReception;
pub use crate::pull_request::reception::minutes_between;
pub use crate::pull_request::reference::PullRequestReference;
pub use crate::pull_request::release::Release;
pub use crate::pull_request::repository::Repository;
pub use crate::pull_request::repository_path::RepositoryPath;
pub use crate::pull_request::review_comment::ReviewComment;
pub use crate::pull_request::review_event::ReviewEvent;
pub use crate::pull_request::review_record::ReviewRecord;
pub use crate::pull_request::review_state::ReviewState;
pub use crate::pull_request::review_tally::ReviewTally;
pub use crate::pull_request::review_tally::tally;
pub use crate::pull_request::review_thread_count::ReviewThreadCount;
pub use crate::pull_request::review_thread_record::ReviewThreadRecord;
pub use crate::pull_request::service::PullRequestService;
pub use crate::pull_request::state::PullRequestState;
pub use crate::pull_request::state_filter::StateFilter;
pub use crate::pull_request::submitted_review::SubmittedReview;
pub use crate::pull_request::thread_comment::ThreadComment;
pub use crate::pull_request::tree::Tree;
pub use crate::pull_request::tree_entry::TreeEntry;
pub use crate::pull_request::tree_entry_kind::TreeEntryKind;
pub use crate::pull_request::webhook_event::WebhookEvent;
pub use crate::pull_request::webhook_signature::WebhookSignature;
pub use crate::pull_request::workflow_job::WorkflowJob;
pub use crate::pull_request::workflow_run::WorkflowRun;
pub use crate::pull_request::workflow_run_filter::WorkflowRunFilter;
