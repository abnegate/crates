use crate::branch_name::BranchName;
use crate::commit_sha::CommitSha;
use crate::conflict::BASE_REF;
use crate::conflict::Conflict;
use crate::conflict::ConflictError;
use crate::conflict::ConflictRequest;
use crate::conflict::ConflictResult;
use crate::conflict::ConflictedPath;
use crate::conflict::HEAD_REF;
use crate::conflict::has_markers;
use crate::conflict::resolve;
use crate::git::authenticate;
use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::path::Path;
use std::process::Stdio;
use tempfile::TempDir;
use tokio::process::Command;

/// The author a repair commits under when the caller names nobody.
const DEFAULT_AUTHOR_NAME: &str = "abnegate-vcs";

/// The address a repair commits under when the caller names nobody.
const DEFAULT_AUTHOR_EMAIL: &str = "abnegate-vcs@localhost";

/// The subdirectory of the throwaway root holding the reproduced merge.
const CHECKOUT_DIRECTORY: &str = "checkout";

/// The subdirectory a repair is given as its home, cache and temporary space.
const ISOLATION_DIRECTORY: &str = "isolation";

/// Reproduces pull request conflicts in throwaway checkouts.
#[derive(Debug, Clone)]
pub struct ConflictService {
    author_name: String,
    author_email: String,
}

impl Default for ConflictService {
    fn default() -> Self {
        Self::new()
    }
}

impl ConflictService {
    pub fn new() -> Self {
        Self {
            author_name: DEFAULT_AUTHOR_NAME.to_string(),
            author_email: DEFAULT_AUTHOR_EMAIL.to_string(),
        }
    }

    /// Commit a repair as `name <email>` rather than as the crate.
    #[must_use]
    pub fn with_author(mut self, name: impl Into<String>, email: impl Into<String>) -> Self {
        self.author_name = name.into();
        self.author_email = email.into();
        self
    }

    /// Fetch both sides into a fresh repository and merge them there.
    ///
    /// Fails with [`ConflictError::NoConflict`] when the merge succeeds, so a
    /// caller can treat a branch that no longer conflicts as nothing to do, and
    /// with [`ConflictError::Moved`] when either side has moved away from what
    /// the caller expected — a repair of a tree nobody asked about is worse than
    /// no repair at all.
    pub async fn reproduce(&self, request: &ConflictRequest) -> ConflictResult<Conflict> {
        let root = TempDir::new()?;
        let checkout = root.path().join(CHECKOUT_DIRECTORY);
        let isolation = root.path().join(ISOLATION_DIRECTORY);
        std::fs::create_dir(&checkout)?;
        std::fs::create_dir(&isolation)?;

        self.run(&checkout, &["init", "--quiet"]).await?;

        let remote = request.remote.clone();

        self.run_authenticated(
            &checkout,
            &[
                "fetch",
                "--no-tags",
                "--quiet",
                &remote,
                &format!("+refs/heads/{}:{HEAD_REF}", request.head),
                &format!("+refs/heads/{}:{BASE_REF}", request.base),
            ],
            request.token.as_deref(),
        )
        .await?;

        let head = self.rev_parse(&checkout, HEAD_REF).await?;
        let base = self.rev_parse(&checkout, BASE_REF).await?;
        expect(&request.expected_head, &head, &request.head)?;
        expect(&request.expected_base, &base, &request.base)?;

        self.run(&checkout, &["checkout", "--detach", "--quiet", HEAD_REF])
            .await?;

        let merged = self
            .attempt(&checkout, &["merge", "--no-commit", "--no-ff", BASE_REF])
            .await?;

        let unmerged = self
            .capture(&checkout, &["diff", "--name-only", "--diff-filter=U", "-z"])
            .await?;

        let mut files = BTreeSet::new();
        for entry in unmerged.split('\0').filter(|entry| !entry.is_empty()) {
            files.insert(ConflictedPath::parse(entry)?);
        }
        let files: Vec<ConflictedPath> = files.into_iter().collect();

        if merged || files.is_empty() {
            return Err(ConflictError::NoConflict);
        }

        for path in &files {
            let resolved = resolve(&checkout, path)?;
            let text = std::fs::read_to_string(&resolved)
                .map_err(|_| ConflictError::NotTextual(path.to_string()))?;
            if !has_markers(&text) {
                return Err(ConflictError::NotTextual(path.to_string()));
            }
        }

        Ok(Conflict {
            root,
            checkout_path: checkout,
            isolation_path: isolation,
            head,
            base,
            files,
        })
    }

    /// Files the working tree changed that the conflict did not name.
    ///
    /// The merge staged everything that combined cleanly, so anything still
    /// showing as modified against the index was touched after the merge — by the
    /// repair. Only the conflicted files have any business being in that list.
    pub async fn strays(&self, conflict: &Conflict) -> ConflictResult<Vec<String>> {
        let modified = self
            .capture(conflict.path(), &["diff", "--name-only", "-z"])
            .await?;

        let named: BTreeSet<&str> = conflict.files().iter().map(|path| path.as_str()).collect();

        let mut strays: BTreeSet<String> = BTreeSet::new();
        for entry in modified.split('\0').filter(|entry| !entry.is_empty()) {
            if !named.contains(entry) {
                strays.insert(entry.to_string());
            }
        }

        Ok(strays.into_iter().collect())
    }

    /// Commit the resolved merge, staging only the conflicted files.
    ///
    /// Everything the merge combined cleanly is already in the index; adding the
    /// conflicted files completes it. Nothing else is staged, so a file the repair
    /// touched outside its scope cannot ride along in the commit.
    pub async fn apply(&self, conflict: &Conflict, message: &str) -> ConflictResult<CommitSha> {
        let mut arguments: Vec<&str> = vec!["add", "--"];
        arguments.extend(conflict.files().iter().map(|path| path.as_str()));
        self.run(conflict.path(), &arguments).await?;

        let name = format!("user.name={}", self.author_name);
        let email = format!("user.email={}", self.author_email);
        self.run(
            conflict.path(),
            &[
                "-c",
                &name,
                "-c",
                &email,
                "commit",
                "--no-verify",
                "--quiet",
                "-m",
                message,
            ],
        )
        .await?;

        self.rev_parse(conflict.path(), "HEAD").await
    }

    /// Push the repaired head, refusing anything that is not a fast-forward.
    ///
    /// A branch somebody else advanced during the repair rejects the push rather
    /// than losing their commits, because nothing here ever forces.
    pub async fn publish(
        &self,
        conflict: &Conflict,
        remote: &str,
        token: Option<&str>,
        branch: &BranchName,
    ) -> ConflictResult<()> {
        let destination = remote.to_string();

        self.run_authenticated(
            conflict.path(),
            &[
                "push",
                "--quiet",
                &destination,
                &format!("HEAD:refs/heads/{branch}"),
            ],
            token,
        )
        .await
    }

    pub(super) async fn rev_parse(
        &self,
        checkout: &Path,
        reference: &str,
    ) -> ConflictResult<CommitSha> {
        let output = self
            .capture(checkout, &["rev-parse", &format!("{reference}^{{commit}}")])
            .await?;
        CommitSha::parse(&output)
    }

    async fn run(&self, checkout: &Path, arguments: &[&str]) -> ConflictResult<()> {
        self.capture(checkout, arguments).await.map(|_| ())
    }

    /// Run a command that reaches the network, authenticating by header.
    async fn run_authenticated(
        &self,
        checkout: &Path,
        arguments: &[&str],
        token: Option<&str>,
    ) -> ConflictResult<()> {
        let mut command = self.command(checkout, arguments);
        if let Some(token) = token {
            authenticate(&mut command, token);
        }
        let output = command.output().await?;
        if !output.status.success() {
            return Err(ConflictError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).trim().to_string(),
            ));
        }
        Ok(())
    }

    async fn capture(&self, checkout: &Path, arguments: &[&str]) -> ConflictResult<String> {
        let output = self.command(checkout, arguments).output().await?;
        if !output.status.success() {
            return Err(ConflictError::CommandFailed(
                String::from_utf8_lossy(&output.stderr).trim().to_string(),
            ));
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    /// Run a merge whose failure is an answer rather than an error.
    ///
    /// Exit 1 is the conflict git was asked to produce. Any other failure —
    /// a missing ref, an unusable identity, a broken checkout — is an error,
    /// and reporting it as "merged cleanly" turns a broken environment into a
    /// silent no-op that looks exactly like a branch needing no repair.
    async fn attempt(&self, checkout: &Path, arguments: &[&str]) -> ConflictResult<bool> {
        let output = self.command(checkout, arguments).output().await?;
        if output.status.success() {
            return Ok(true);
        }
        if output.status.code() == Some(1) {
            return Ok(false);
        }
        Err(ConflictError::CommandFailed(format!(
            "git {} exited with {}: {}",
            arguments.join(" "),
            output
                .status
                .code()
                .map_or_else(|| "a signal".to_string(), |code| code.to_string()),
            String::from_utf8_lossy(&output.stderr).trim()
        )))
    }

    fn command(&self, checkout: &Path, arguments: &[&str]) -> Command {
        let mut command = Command::new("git");
        command
            .current_dir(checkout)
            .env_clear()
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_ASKPASS", "")
            .env("HOME", checkout)
            .env("LC_ALL", "C")
            // env_clear removed any identity and the global config is
            // /dev/null, so git has none to fall back on. A host whose git
            // cannot synthesise one from the passwd entry refuses to merge or
            // commit at all, which is most CI runners.
            .env("GIT_AUTHOR_NAME", &self.author_name)
            .env("GIT_AUTHOR_EMAIL", &self.author_email)
            .env("GIT_COMMITTER_NAME", &self.author_name)
            .env("GIT_COMMITTER_EMAIL", &self.author_email)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        if let Some(path) = std::env::var_os("PATH") {
            command.env("PATH", path);
        }
        for argument in arguments {
            command.arg(OsStr::new(argument));
        }
        command
    }
}

fn expect(
    expected: &Option<CommitSha>,
    actual: &CommitSha,
    branch: &BranchName,
) -> ConflictResult<()> {
    match expected {
        Some(expected) if expected != actual => Err(ConflictError::Moved {
            branch: branch.to_string(),
            expected: expected.to_string(),
            actual: actual.to_string(),
        }),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The identity a repair commits under reaches git through the environment
    /// as well as the command line, so a caller that named one is obeyed by
    /// both the merge and the commit.
    #[test]
    fn the_author_a_repair_commits_under_is_the_callers_to_name() {
        let service = ConflictService::new().with_author("Ada", "ada@example.test");
        let command = service.command(Path::new("."), &["status"]);
        let environment: Vec<(String, String)> = command
            .as_std()
            .get_envs()
            .filter_map(|(key, value)| {
                Some((
                    key.to_string_lossy().into_owned(),
                    value?.to_string_lossy().into_owned(),
                ))
            })
            .collect();

        assert!(
            environment.contains(&("GIT_AUTHOR_NAME".to_string(), "Ada".to_string())),
            "{environment:?}"
        );
        assert!(
            environment.contains(&(
                "GIT_COMMITTER_EMAIL".to_string(),
                "ada@example.test".to_string()
            )),
            "{environment:?}"
        );
    }
}
