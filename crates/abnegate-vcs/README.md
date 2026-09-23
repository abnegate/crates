# abnegate-vcs

Version control over the `git` command line. `GitService` shells out to `git`
for the clone, branch, commit, push and worktree a change needs; `worktree` adds
and removes the detached worktrees a batch of concurrent runs works in;
`ConflictService` reproduces a pull request's merge conflict in a throwaway
checkout and `resolution::judge` refuses a repair that threw a branch's work
away; `Subject` names a change the way a conventional-commit history names one;
and `DependencyDiscovery` reads package manifests for the dependencies one
organisation has on itself. With the `github` feature, `PullRequestService`
opens and reads back pull requests over the GitHub REST API. Nothing here holds
application state, so a task runner can drive it directly.

## Requirements

git 2.39 or newer, the first to read a symbolic ref without following it. On an
older git, `GitService::commit` fails closed and `worktree::branch` reads a
worktree's branch as unknown. The test suite needs git 2.45 or newer, the first
to take `--ref-format` for `init` and `clone`.

## Features

- `github`: `PullRequestService`, which opens pull requests on GitHub or a GitHub Enterprise install and reads back how each one was received.
- `test-support`: `RepositoryUrl::local` and `PullRequestService::standing_in_for`, which reach a repository on the local disk or a mock API server; only a test should enable it.

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
