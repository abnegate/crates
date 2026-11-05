# abnegate-vcs

Version control over the `git` command line. `GitService` shells out to `git`
for the clone, branch, commit, push and worktree a change needs, in a `Checkout`
named by its top and bound to the clone it was made from; `worktree` adds and
removes the detached worktrees a batch of concurrent runs works in;
`ConflictService` reproduces a pull request's merge conflict in a throwaway
checkout, or refreshes a branch by merging its base in there and pushing the
merge, and `resolution::judge` refuses a repair that threw a branch's work
away; `Subject` names a change the way a conventional-commit history names one;
and `DependencyDiscovery` reads package manifests for the dependencies one
organisation has on itself. With the `github` feature, `PullRequestService`
opens, reads, checks, reviews and merges pull requests, sets them to merge once
they are ready, moves them in and out of draft, creates repositories and
registers their webhooks, over GitHub's REST and GraphQL APIs. Nothing here holds
application state, so a task runner can drive it directly.

## Requirements

git 2.39 or newer, the first to read a symbolic ref without following it. On an
older git, `GitService::commit` fails closed and `worktree::branch` reads a
worktree's branch as unknown. The test suite needs git 2.45 or newer, the first
to take `--ref-format` for `init` and `clone`.

## Features

- `github`: `PullRequestService`, which opens, reads, checks, reviews and merges pull requests on GitHub or a GitHub Enterprise install, sets them to merge once they are ready and moves them in and out of draft, reads back how each one was received, and creates repositories there and registers their webhooks, over the REST and GraphQL APIs.
- `github-app`: `github`, plus what authenticating as a GitHub App installation takes: `Claims` signs the RS256 JWT that proves the caller is the App, `Issuer` trades it for an installation access token without ever following a redirect, and `TokenCache` holds each installation's token until it comes within five minutes of expiring.
- `testing`: `RepositoryUrl::local`, `PullRequestService::standing_in_for` and, with `github-app`, `Issuer::standing_in_for`, which reach a repository on the local disk or a mock API server; only a test should enable it.

## Usage

```sh
cargo add abnegate-vcs
cargo add uuid --features v4
```

```rust,no_run
use std::path::Path;

use abnegate_vcs::{GitError, GitService};
use uuid::Uuid;

async fn name_branch(repository: &Path) -> Result<(), GitError> {
    let git = GitService::new();
    if git.is_git_repository(repository).await? {
        let branch = git.generate_branch_name(Uuid::new_v4(), "add rate limiting")?;
        println!("{branch}");
    }
    Ok(())
}
```
