//! Reproducing a pull request's merge conflict in a throwaway checkout.
//!
//! A repair is only worth running against the exact tree that conflicts. Rather
//! than reuse the workspace a run left behind — which has a branch checked out,
//! may have moved on, and is shared with whatever else is running — this builds a
//! fresh repository in a temporary directory, fetches only the two commits under
//! discussion, and reproduces the merge there. The directory is deleted when the
//! [`Conflict`] is dropped.
//!
//! Nothing here ever touches an existing repository, and nothing here ever uses
//! the stash: `refs/stash` is shared between worktrees, so a background repair
//! that stashed would corrupt whatever else was running beside it.

use crate::git::authenticate;
use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;
use std::process::Stdio;
use tempfile::TempDir;
use thiserror::Error;
use tokio::process::Command;

/// The ref a fetched head lands on inside the throwaway checkout.
const HEAD_REF: &str = "refs/conflict/head";

/// The ref a fetched base lands on inside the throwaway checkout.
const BASE_REF: &str = "refs/conflict/base";

/// The line a conflicted file opens each hunk with.
pub const OURS_MARKER: &str = "<<<<<<<";

/// The line diff3-style conflicts use to introduce the merge base.
pub const BASE_MARKER: &str = "|||||||";

/// The line separating the two sides of a conflict hunk.
pub const SPLIT_MARKER: &str = "=======";

/// The line a conflicted file closes each hunk with.
pub const THEIRS_MARKER: &str = ">>>>>>>";

/// The author a repair commits under when the caller names nobody.
const DEFAULT_AUTHOR_NAME: &str = "abnegate-vcs";

/// The address a repair commits under when the caller names nobody.
const DEFAULT_AUTHOR_EMAIL: &str = "abnegate-vcs@localhost";

/// Longest ref name git itself will accept without complaint.
const MAX_BRANCH_LENGTH: usize = 255;

/// The subdirectory of the throwaway root holding the reproduced merge.
const CHECKOUT_DIRECTORY: &str = "checkout";

/// The subdirectory a repair is given as its home, cache and temporary space.
const ISOLATION_DIRECTORY: &str = "isolation";

#[derive(Debug, Error)]
pub enum ConflictError {
    #[error("Git command failed: {0}")]
    CommandFailed(String),

    #[error("Invalid commit identifier: {0}")]
    InvalidCommit(String),

    #[error("Invalid branch name: {0}")]
    InvalidBranch(String),

    #[error("Unsafe conflicted path: {0}")]
    UnsafePath(String),

    #[error("{branch} is at {actual}, not the expected {expected}")]
    Moved {
        branch: String,
        expected: String,
        actual: String,
    },

    #[error("The branch merges cleanly; there is no conflict to repair")]
    NoConflict,

    #[error("Conflicted file {0} carries no conflict markers and cannot be repaired as text")]
    NotTextual(String),

    #[error("The checkout no longer holds the conflicted state it was prepared with")]
    CheckoutMoved,

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

pub type ConflictResult<T> = Result<T, ConflictError>;

/// A full commit identifier, lowercased.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CommitSha(String);

impl CommitSha {
    pub fn parse(value: &str) -> ConflictResult<Self> {
        let trimmed = value.trim();
        if trimmed.len() != 40 || !trimmed.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(ConflictError::InvalidCommit(value.to_string()));
        }
        Ok(Self(trimmed.to_ascii_lowercase()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for CommitSha {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// A branch name git will accept and a shell cannot reinterpret.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BranchName(String);

impl BranchName {
    pub fn parse(value: &str) -> ConflictResult<Self> {
        let refused = value.is_empty()
            || value.len() > MAX_BRANCH_LENGTH
            || value.starts_with('-')
            || value.starts_with('/')
            || value.ends_with('/')
            || value.ends_with('.')
            || value.contains("..")
            || value.contains("@{")
            || value.contains("//")
            || value.contains(['~', '^', ':', '?', '*', '[', '\\', ' '])
            || value.chars().any(|character| character.is_control())
            || value
                .split('/')
                .any(|part| part.is_empty() || part.starts_with('.'));

        if refused {
            return Err(ConflictError::InvalidBranch(value.to_string()));
        }
        Ok(Self(value.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for BranchName {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// A repository-relative path git reported as unmerged.
///
/// Slash separated, never absolute, never leaving the repository, and free of
/// the characters that turn a path into a glob or an option.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ConflictedPath(String);

impl ConflictedPath {
    pub fn parse(value: &str) -> ConflictResult<Self> {
        let refused = value.is_empty()
            || value.starts_with('/')
            || value.starts_with('-')
            || value.contains('\\')
            || value.contains("//")
            || value.bytes().any(|byte| {
                byte <= b' '
                    || byte == 0x7f
                    || matches!(
                        byte,
                        b',' | b'(' | b')' | b'*' | b'?' | b'[' | b']' | b'{' | b'}'
                    )
            })
            || value
                .split('/')
                .any(|part| part.is_empty() || part == "." || part == "..");

        if refused {
            return Err(ConflictError::UnsafePath(value.to_string()));
        }
        Ok(Self(value.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ConflictedPath {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Resolve one repository-relative path to a real file inside `checkout`.
///
/// Every component is inspected: a symlink anywhere along the way, a final
/// component that is not a regular file, or a canonical path that leaves the
/// checkout all mean the path cannot be repaired safely.
pub fn resolve(checkout: &Path, path: &ConflictedPath) -> ConflictResult<PathBuf> {
    let root = checkout
        .canonicalize()
        .map_err(|_| ConflictError::UnsafePath(path.to_string()))?;

    let mut target = root.clone();
    let components: Vec<&str> = path.as_str().split('/').collect();
    for (index, component) in components.iter().enumerate() {
        target.push(component);
        let details = std::fs::symlink_metadata(&target)
            .map_err(|_| ConflictError::UnsafePath(path.to_string()))?;
        let last = index + 1 == components.len();
        let acceptable = match last {
            true => details.is_file(),
            false => details.is_dir(),
        };
        if details.file_type().is_symlink() || !acceptable {
            return Err(ConflictError::UnsafePath(path.to_string()));
        }
    }

    let canonical = target
        .canonicalize()
        .map_err(|_| ConflictError::UnsafePath(path.to_string()))?;
    if canonical != target || !canonical.starts_with(&root) {
        return Err(ConflictError::UnsafePath(path.to_string()));
    }

    Ok(canonical)
}

/// Resolve every conflicted path, refusing the whole set if any one is unsafe.
pub fn validate(checkout: &Path, files: &[ConflictedPath]) -> ConflictResult<Vec<PathBuf>> {
    if files.is_empty() {
        return Err(ConflictError::NoConflict);
    }
    files.iter().map(|path| resolve(checkout, path)).collect()
}

/// Whether a file's text still carries the markers git wrote into it.
pub fn has_markers(text: &str) -> bool {
    text.lines()
        .any(|line| line.starts_with(OURS_MARKER) || line.starts_with(THEIRS_MARKER))
}

/// What to reproduce, and what the caller believes it should reproduce to.
#[derive(Debug, Clone)]
pub struct ConflictRequest {
    pub remote: String,
    pub token: Option<String>,
    pub head: BranchName,
    pub base: BranchName,
    pub expected_head: Option<CommitSha>,
    pub expected_base: Option<CommitSha>,
}

/// A reproduced conflict, and the throwaway directory holding it.
///
/// Dropping this deletes both the checkout and the isolation directory a repair
/// was pointed at, so a repair cannot leave a half-merged tree or a stray home
/// directory behind on disk.
#[derive(Debug)]
pub struct Conflict {
    #[allow(
        dead_code,
        reason = "held so the throwaway directory outlives the repair"
    )]
    root: TempDir,
    checkout_path: PathBuf,
    isolation_path: PathBuf,
    head: CommitSha,
    base: CommitSha,
    files: Vec<ConflictedPath>,
}

impl Conflict {
    pub fn path(&self) -> &Path {
        &self.checkout_path
    }

    /// The directory a repair may use as its home, cache and temporary space.
    pub fn isolation(&self) -> &Path {
        &self.isolation_path
    }

    pub fn head(&self) -> &CommitSha {
        &self.head
    }

    pub fn base(&self) -> &CommitSha {
        &self.base
    }

    pub fn files(&self) -> &[ConflictedPath] {
        &self.files
    }

    /// The absolute path of one conflicted file, refusing anything outside the set.
    ///
    /// The candidate may be repository-relative or absolute inside the checkout;
    /// either way it has to name a file git reported as unmerged.
    pub fn confine(&self, candidate: &str) -> ConflictResult<PathBuf> {
        let relative = self.relative(candidate)?;
        if !self.files.contains(&relative) {
            return Err(ConflictError::UnsafePath(candidate.to_string()));
        }
        resolve(self.path(), &relative)
    }

    fn relative(&self, candidate: &str) -> ConflictResult<ConflictedPath> {
        let trimmed = candidate.trim();
        if trimmed.is_empty() {
            return Err(ConflictError::UnsafePath(candidate.to_string()));
        }

        let as_path = Path::new(trimmed);
        let within = match as_path.is_absolute() {
            true => {
                let root = self
                    .path()
                    .canonicalize()
                    .map_err(|_| ConflictError::UnsafePath(candidate.to_string()))?;
                let normalized = normalize(as_path)
                    .ok_or_else(|| ConflictError::UnsafePath(candidate.to_string()))?;
                normalized
                    .strip_prefix(&root)
                    .map_err(|_| ConflictError::UnsafePath(candidate.to_string()))?
                    .to_path_buf()
            }
            false => PathBuf::from(trimmed),
        };

        let text = within
            .to_str()
            .ok_or_else(|| ConflictError::UnsafePath(candidate.to_string()))?
            .replace(std::path::MAIN_SEPARATOR, "/");
        ConflictedPath::parse(&text)
    }

    /// Confirm the checkout still holds the exact commits it was prepared with.
    ///
    /// Run after an agent has edited the tree: a repair that was applied to a
    /// checkout somebody moved underneath it is not a repair of this conflict.
    pub async fn verify(&self, service: &ConflictService) -> ConflictResult<()> {
        let checked_out = service.rev_parse(self.path(), "HEAD").await?;
        let head = service.rev_parse(self.path(), HEAD_REF).await?;
        let base = service.rev_parse(self.path(), BASE_REF).await?;

        if checked_out != self.head || head != self.head || base != self.base {
            return Err(ConflictError::CheckoutMoved);
        }
        Ok(())
    }
}

/// Lexically resolve `.` and `..` without touching the filesystem.
fn normalize(path: &Path) -> Option<PathBuf> {
    let mut resolved = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                if !resolved.pop() {
                    return None;
                }
            }
            Component::CurDir => {}
            other => resolved.push(other),
        }
    }
    Some(resolved)
}

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

    async fn rev_parse(&self, checkout: &Path, reference: &str) -> ConflictResult<CommitSha> {
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

    #[test]
    fn a_commit_identifier_must_be_forty_hex_digits() {
        let parsed = CommitSha::parse("A1B2C3D4E5F60718293A4B5C6D7E8F90A1B2C3D4").unwrap();
        assert_eq!(parsed.as_str(), "a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4");
        assert_eq!(parsed.to_string(), parsed.as_str());

        for invalid in ["", "abc", "z1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4"] {
            assert!(
                CommitSha::parse(invalid).is_err(),
                "{invalid} is not a commit"
            );
        }
    }

    #[test]
    fn a_branch_name_may_not_be_an_option_or_an_escape() {
        assert!(BranchName::parse("feature/one").is_ok());
        for invalid in [
            "",
            "-force",
            "/leading",
            "trailing/",
            "a..b",
            "a@{1}",
            "a//b",
            "a b",
            "a^b",
            "a:b",
            "a*b",
            "a\\b",
            ".hidden",
            "dir/.hidden",
        ] {
            assert!(
                BranchName::parse(invalid).is_err(),
                "{invalid:?} must not be accepted as a branch"
            );
        }
        assert!(
            BranchName::parse(&"a".repeat(MAX_BRANCH_LENGTH + 1)).is_err(),
            "a name longer than git accepts is not a branch"
        );
    }

    #[test]
    fn a_conflicted_path_must_stay_inside_the_repository() {
        assert!(ConflictedPath::parse("src/main.rs").is_ok());
        for invalid in [
            "",
            "/etc/passwd",
            "../outside",
            "src/../../outside",
            "src/./main.rs",
            "src\\main.rs",
            "src/*.rs",
            "src/ma in.rs",
            "-oops",
        ] {
            assert!(
                ConflictedPath::parse(invalid).is_err(),
                "{invalid:?} must not be accepted as a conflicted path"
            );
        }
    }

    #[test]
    fn markers_are_recognised_only_at_the_start_of_a_line() {
        assert!(has_markers(
            "a\n<<<<<<< ours\nb\n=======\nc\n>>>>>>> theirs\n"
        ));
        assert!(!has_markers("a diff shows <<<<<<< inline\n"));
        assert!(!has_markers("plain text"));
    }

    #[test]
    fn an_empty_conflicted_set_is_not_a_conflict() {
        let root = TempDir::new().unwrap();
        assert!(matches!(
            validate(root.path(), &[]),
            Err(ConflictError::NoConflict)
        ));
    }

    #[test]
    fn a_symlinked_conflicted_path_is_refused() {
        let root = TempDir::new().unwrap();
        std::fs::write(root.path().join("real.txt"), "content").unwrap();

        #[cfg(unix)]
        std::os::unix::fs::symlink(root.path().join("real.txt"), root.path().join("link.txt"))
            .unwrap();

        let path = ConflictedPath::parse("link.txt").unwrap();
        assert!(
            matches!(
                resolve(root.path(), &path),
                Err(ConflictError::UnsafePath(_))
            ),
            "a symlink can point outside the checkout and must never be repaired"
        );
    }

    #[test]
    fn a_directory_is_not_a_conflicted_file() {
        let root = TempDir::new().unwrap();
        std::fs::create_dir(root.path().join("src")).unwrap();
        let path = ConflictedPath::parse("src").unwrap();
        assert!(matches!(
            resolve(root.path(), &path),
            Err(ConflictError::UnsafePath(_))
        ));
    }

    #[test]
    fn a_validated_set_resolves_every_member() {
        let root = TempDir::new().unwrap();
        std::fs::create_dir(root.path().join("src")).unwrap();
        std::fs::write(root.path().join("src/main.rs"), "fn main() {}").unwrap();
        std::fs::write(root.path().join("README.md"), "docs").unwrap();

        let files = vec![
            ConflictedPath::parse("src/main.rs").unwrap(),
            ConflictedPath::parse("README.md").unwrap(),
        ];
        let resolved = validate(root.path(), &files).unwrap();
        assert_eq!(resolved.len(), 2);
        assert!(resolved.iter().all(|path| path.is_file()));
    }

    #[test]
    fn one_unsafe_member_refuses_the_whole_set() {
        let root = TempDir::new().unwrap();
        std::fs::write(root.path().join("present.txt"), "here").unwrap();

        let files = vec![
            ConflictedPath::parse("present.txt").unwrap(),
            ConflictedPath::parse("absent.txt").unwrap(),
        ];
        assert!(
            validate(root.path(), &files).is_err(),
            "a set is only as safe as its least safe member"
        );
    }

    #[test]
    fn a_path_is_resolved_lexically_or_refused_for_climbing_out() {
        assert_eq!(
            normalize(Path::new("/a/./b/../c")),
            Some(PathBuf::from("/a/c"))
        );
        assert_eq!(normalize(Path::new("a/../..")), None);
    }

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
