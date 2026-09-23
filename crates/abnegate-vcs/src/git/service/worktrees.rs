use super::*;
use crate::git::WorktreeEntry;
use crate::worktree;
use std::path::Component;

/// The suffix a directory holding worktrees has to carry before
/// [`GitService::remove_worktree`] will delete anything inside it by hand.
const WORKTREE_AREA_SUFFIX: &str = "-worktrees";

/// What marks the top of a work tree.
const GIT_MARKER: &str = ".git";

/// How a worktree's `.git` file introduces the directory git keeps it in.
const GIT_POINTER: &str = "gitdir: ";

/// The directory of a repository's own records of its worktrees.
const WORKTREE_RECORDS: &str = "worktrees";

fn refuse(worktree_path: &Path) -> GitError {
    GitError::UnsafeWorktree(worktree_path.to_path_buf())
}

impl GitService {
    /// Add a worktree of a managed clone at `worktree_path`, in detached HEAD
    /// state at `checkout_ref`.
    ///
    /// A worktree of the same repository already standing there is replaced
    /// when it holds nothing that would be lost -- no uncommitted change and a
    /// HEAD at the commit it is about to be recreated on -- so one a crashed
    /// run left behind does not refuse the next. Anything else standing there
    /// is refused with [`GitError::UnsafeWorktree`].
    ///
    /// The clone's own configuration is verified and the worktree commands are
    /// hardened first, because a worktree of a managed clone shares that
    /// configuration and a run works in a worktree: a clone whose
    /// configuration a run left something no pin reaches in is refused with
    /// [`GitError::UnsafeConfig`] before a worktree is added.
    pub async fn create_worktree(
        &self,
        path: &Path,
        worktree_path: &Path,
        checkout_ref: &BranchName,
    ) -> GitResult<()> {
        Self::verify_config(path).await?;
        let worktree_path = Self::make_absolute(worktree_path)?;
        let worktree_path = worktree_path.as_path();

        if worktree_path.exists() {
            self.replace_stale(path, worktree_path, checkout_ref)
                .await?;
        }

        if let Some(parent) = worktree_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        tracing::info!(
            repository = ?path,
            worktree = ?worktree_path,
            %checkout_ref,
            "Creating worktree"
        );

        let output = Self::output(
            Self::managed_local(path)
                .args(["worktree", "add", "--detach", "--"])
                .arg(worktree_path)
                .arg(checkout_ref.as_str()),
        )
        .await?;

        if !output.status.success() {
            return Err(GitError::CommandFailed(format!(
                "git worktree add failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }

        tracing::info!(worktree = ?worktree_path, "Worktree created");
        Ok(())
    }

    /// Add a worktree of a managed clone checked out on a local branch.
    ///
    /// Unlike [`Self::create_worktree`], which detaches, this creates or resets
    /// `branch` at `start_point`, so a later push from the worktree targets the
    /// branch the caller named. A worktree already standing there is replaced
    /// or refused as [`Self::create_worktree`] replaces or refuses one.
    pub async fn create_worktree_on_branch(
        &self,
        path: &Path,
        worktree_path: &Path,
        branch: &BranchName,
        start_point: &BranchName,
    ) -> GitResult<()> {
        Self::verify_config(path).await?;
        let worktree_path = Self::make_absolute(worktree_path)?;
        let worktree_path = worktree_path.as_path();

        if worktree_path.exists() {
            self.replace_stale(path, worktree_path, start_point).await?;
        }

        if let Some(parent) = worktree_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        tracing::info!(
            repository = ?path,
            worktree = ?worktree_path,
            %branch,
            %start_point,
            "Creating worktree on branch"
        );

        let output = Self::output(
            Self::managed_local(path)
                .args(["worktree", "add", "-B", branch.as_str(), "--"])
                .arg(worktree_path)
                .arg(start_point.as_str()),
        )
        .await?;

        if !output.status.success() {
            return Err(GitError::CommandFailed(format!(
                "git worktree add -B {branch} failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }

        tracing::info!(worktree = ?worktree_path, %branch, "Worktree created on branch");
        Ok(())
    }

    /// Remove a worktree of a managed clone and prune the record of it.
    ///
    /// A directory git refuses to let go of is deleted by hand only when it is
    /// certainly a worktree nobody declared kept: a real directory, not a
    /// link, directly inside a real `*-worktrees` directory reached without
    /// `.` or `..`, registered with the repository as one of its worktrees,
    /// not the repository itself, and not locked. Everything else is somewhere
    /// the caller did not declare disposable, and is refused.
    pub async fn remove_worktree(&self, path: &Path, worktree_path: &Path) -> GitResult<()> {
        Self::verify_config(path).await?;
        let worktree_path = Self::make_absolute(worktree_path)?;
        let worktree_path = worktree_path.as_path();
        if worktree_path.components().any(|component| {
            !matches!(
                component,
                Component::Normal(_) | Component::RootDir | Component::Prefix(_)
            )
        }) {
            return Err(GitError::UnsafeWorktree(worktree_path.to_path_buf()));
        }

        tracing::debug!(repository = ?path, worktree = ?worktree_path, "Removing worktree");

        let disposable = match std::fs::symlink_metadata(worktree_path) {
            Ok(_) => Some(self.disposable(path, worktree_path).await),
            Err(_) => None,
        };

        let output = Self::output(
            Self::managed_local(path)
                .args(["worktree", "remove", "--force", "--"])
                .arg(worktree_path),
        )
        .await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            tracing::warn!(error = %stderr, "git worktree remove failed");
        }

        if std::fs::symlink_metadata(worktree_path).is_ok() {
            let disposable = disposable.unwrap_or_else(|| Err(refuse(worktree_path)))?;
            tracing::warn!(worktree = ?disposable, "Deleting a worktree git would not remove");
            tokio::fs::remove_dir_all(&disposable).await?;
        }

        let _ = Self::output(Self::managed_local(path).args(["worktree", "prune"])).await;

        tracing::debug!(worktree = ?worktree_path, "Worktree removed");
        Ok(())
    }

    /// The real path of `worktree_path` when it may be deleted by hand, as
    /// [`Self::remove_worktree`] describes. Decided before git is asked to
    /// remove it, because git forgets a worktree it could not finish deleting
    /// and the registration is the evidence.
    async fn disposable(&self, path: &Path, worktree_path: &Path) -> GitResult<PathBuf> {
        let refuse = || refuse(worktree_path);
        let details = std::fs::symlink_metadata(worktree_path)?;
        if details.file_type().is_symlink() || !details.is_dir() {
            return Err(refuse());
        }
        let (Some(parent), Some(name)) = (worktree_path.parent(), worktree_path.file_name()) else {
            return Err(refuse());
        };
        let area = parent.canonicalize()?;
        let in_area = area
            .file_name()
            .is_some_and(|area| area.to_string_lossy().ends_with(WORKTREE_AREA_SUFFIX));
        let target = area.join(name);
        let repository = path.canonicalize()?;
        if !in_area || repository.starts_with(&target) {
            return Err(refuse());
        }

        let listed =
            Self::output(Self::managed_local(path).args(["worktree", "list", "--porcelain", "-z"]))
                .await?;
        if !listed.status.success() {
            return Err(refuse());
        }
        let mut registered = None;
        for entry in WorktreeEntry::parse(&listed.stdout).into_iter().skip(1) {
            let real = entry
                .path
                .canonicalize()
                .unwrap_or_else(|_| entry.path.clone());
            if real == target {
                registered = Some(entry);
            } else if real.starts_with(&target) {
                return Err(refuse());
            }
        }
        match registered {
            Some(entry) if !entry.locked && self.belongs(path, &target).await? => Ok(target),
            _ => Err(refuse()),
        }
    }

    /// Whether the directory at `target` is this repository's worktree rather
    /// than something else that now stands where one was registered: its
    /// `.git` is either gone, as a crashed run leaves it, or a file pointing
    /// into this repository's own worktree records.
    async fn belongs(&self, path: &Path, target: &Path) -> GitResult<bool> {
        let marker = target.join(GIT_MARKER);
        let Ok(details) = std::fs::symlink_metadata(&marker) else {
            return Ok(true);
        };
        if !details.is_file() {
            return Ok(false);
        }
        let common = Self::output(Self::managed_local(path).args([
            "rev-parse",
            "--path-format=absolute",
            "--git-common-dir",
        ]))
        .await?;
        if !common.status.success() {
            return Ok(false);
        }
        let records = PathBuf::from(String::from_utf8_lossy(&common.stdout).trim())
            .join(WORKTREE_RECORDS)
            .canonicalize()?;
        let pointer = std::fs::read_to_string(&marker)?;
        let recorded = pointer
            .trim()
            .strip_prefix(GIT_POINTER)
            .map(|recorded| target.join(recorded))
            .and_then(|recorded| recorded.canonicalize().ok());
        Ok(recorded.is_some_and(|recorded| recorded.starts_with(&records)))
    }

    /// Clear the way for a worktree at `worktree_path` by removing the one
    /// standing there, but only when it is a worktree of this repository that
    /// holds nothing: no uncommitted change, and a HEAD at `start`.
    async fn replace_stale(
        &self,
        path: &Path,
        worktree_path: &Path,
        start: &BranchName,
    ) -> GitResult<()> {
        let refuse = || GitError::UnsafeWorktree(worktree_path.to_path_buf());
        let resolved = Self::output(Self::managed_local(path).args([
            "rev-parse",
            "--verify",
            "--end-of-options",
            &format!("{start}^{{commit}}"),
        ]))
        .await?;
        if !resolved.status.success() {
            return Err(refuse());
        }
        let start = String::from_utf8_lossy(&resolved.stdout).trim().to_string();
        let repository = path.canonicalize()?;
        let stale = worktree_path.to_path_buf();
        let holds_nothing = tokio::task::spawn_blocking(move || {
            worktree::is_worktree(&stale)
                && worktree::repository_of(&stale).is_ok_and(|owner| owner == repository)
                && worktree::unfinished(&stale, &[&start]).is_ok_and(|held| !held.any())
        })
        .await
        .map_err(std::io::Error::other)?;
        if !holds_nothing {
            return Err(refuse());
        }

        tracing::warn!(worktree = ?worktree_path, "Stale worktree found, removing");
        self.remove_worktree(path, worktree_path).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::service::hardened::fixtures::branch;
    use crate::worktree::fixtures::git;
    use crate::worktree::fixtures::remote;
    use tempfile::TempDir;

    struct Fixture {
        root: TempDir,
        repository: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let root = TempDir::new().unwrap();
            let repository = root.path().join("repository");
            std::fs::create_dir(&repository).unwrap();
            remote(&repository);
            Self { root, repository }
        }

        fn at(&self, relative: &str) -> PathBuf {
            self.root.path().join(relative)
        }

        fn victim(&self) -> PathBuf {
            let victim = self.at("victim");
            std::fs::create_dir_all(&victim).unwrap();
            std::fs::write(victim.join("precious"), "keep\n").unwrap();
            victim
        }

        async fn worktree(&self, relative: &str) -> PathBuf {
            let path = self.at(relative);
            GitService::new()
                .create_worktree(&self.repository, &path, &branch("main"))
                .await
                .unwrap();
            path
        }
    }

    async fn refused(fixture: &Fixture, worktree: &Path) -> bool {
        matches!(
            GitService::new()
                .remove_worktree(&fixture.repository, worktree)
                .await,
            Err(GitError::UnsafeWorktree(_))
        )
    }

    #[tokio::test]
    async fn a_path_that_climbs_out_of_the_area_is_refused() {
        let fixture = Fixture::new();
        std::fs::create_dir_all(fixture.at("area-worktrees")).unwrap();

        for climbing in [
            "area-worktrees/..",
            "area-worktrees/../victim",
            "victim/../area-worktrees/one",
        ] {
            fixture.victim();
            assert!(refused(&fixture, &fixture.at(climbing)).await, "{climbing}");
        }
        assert!(fixture.repository.join("README").exists());
        assert!(fixture.at("victim/precious").exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn an_area_that_is_a_link_to_somewhere_else_is_refused() {
        let fixture = Fixture::new();
        let victim = fixture.victim();
        std::fs::create_dir(victim.join("one")).unwrap();
        std::fs::write(victim.join("one").join("precious"), "keep\n").unwrap();
        std::os::unix::fs::symlink(&victim, fixture.at("linked-worktrees")).unwrap();

        assert!(refused(&fixture, &fixture.at("linked-worktrees/one")).await);
        assert!(victim.join("one").join("precious").exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_worktree_path_that_is_a_link_is_refused() {
        let fixture = Fixture::new();
        let victim = fixture.victim();
        std::fs::create_dir_all(fixture.at("area-worktrees")).unwrap();
        std::os::unix::fs::symlink(&victim, fixture.at("area-worktrees/one")).unwrap();

        assert!(refused(&fixture, &fixture.at("area-worktrees/one")).await);
        assert!(victim.join("precious").exists());
        assert!(std::fs::symlink_metadata(fixture.at("area-worktrees/one")).is_ok());
    }

    #[tokio::test]
    async fn a_directory_the_repository_never_registered_is_refused() {
        let fixture = Fixture::new();
        let plain = fixture.at("area-worktrees/plain");
        std::fs::create_dir_all(&plain).unwrap();
        std::fs::write(plain.join("precious"), "keep\n").unwrap();

        assert!(refused(&fixture, &plain).await);
        assert!(plain.join("precious").exists());
    }

    #[tokio::test]
    async fn a_repository_that_lives_in_an_area_is_never_removed_as_a_worktree_of_itself() {
        let root = TempDir::new().unwrap();
        let repository = root.path().join("home-worktrees").join("main");
        std::fs::create_dir_all(&repository).unwrap();
        remote(&repository);

        let refusal = GitService::new()
            .remove_worktree(&repository, &repository)
            .await;

        assert!(
            matches!(refusal, Err(GitError::UnsafeWorktree(_))),
            "{refusal:?}"
        );
        assert!(repository.join("README").exists());
    }

    #[tokio::test]
    async fn a_locked_worktree_is_never_deleted_by_hand() {
        let fixture = Fixture::new();
        let worktree = fixture.worktree("area-worktrees/locked").await;
        git(
            &fixture.repository,
            &[
                "worktree",
                "lock",
                "--reason",
                "kept",
                worktree.to_str().unwrap(),
            ],
        );

        assert!(refused(&fixture, &worktree).await);
        assert!(worktree.join("README").exists());
    }

    /// A registered worktree git cannot remove -- its `.git` file gone -- is
    /// still one a crashed run left behind, and is deleted by hand.
    #[tokio::test]
    async fn a_registered_worktree_git_cannot_remove_is_deleted_by_hand() {
        let fixture = Fixture::new();
        let worktree = fixture.worktree("area-worktrees/broken").await;
        std::fs::remove_file(worktree.join(".git")).unwrap();

        GitService::new()
            .remove_worktree(&fixture.repository, &worktree)
            .await
            .unwrap();

        assert!(!worktree.exists());
        assert!(
            !git(&fixture.repository, &["worktree", "list"]).contains("broken"),
            "the registration was pruned"
        );
    }

    #[tokio::test]
    async fn a_worktree_holding_uncommitted_work_is_not_replaced() {
        let fixture = Fixture::new();
        let worktree = fixture.worktree("area-worktrees/one").await;
        std::fs::write(worktree.join("work.txt"), "unsaved\n").unwrap();

        let refusal = GitService::new()
            .create_worktree(&fixture.repository, &worktree, &branch("main"))
            .await;

        assert!(
            matches!(refusal, Err(GitError::UnsafeWorktree(_))),
            "{refusal:?}"
        );
        assert!(worktree.join("work.txt").exists());
    }

    #[tokio::test]
    async fn a_worktree_holding_a_commit_elsewhere_is_not_replaced() {
        let fixture = Fixture::new();
        let worktree = fixture.worktree("area-worktrees/one").await;
        std::fs::write(worktree.join("work.txt"), "committed\n").unwrap();
        git(&worktree, &["add", "work.txt"]);
        git(&worktree, &["commit", "-q", "-m", "work"]);

        let refusal = GitService::new()
            .create_worktree_on_branch(
                &fixture.repository,
                &worktree,
                &branch("task"),
                &branch("main"),
            )
            .await;

        assert!(
            matches!(refusal, Err(GitError::UnsafeWorktree(_))),
            "{refusal:?}"
        );
        assert!(worktree.join("work.txt").exists());
    }

    #[tokio::test]
    async fn anything_standing_where_a_worktree_goes_that_is_not_one_of_ours_is_not_replaced() {
        let fixture = Fixture::new();
        let plain = fixture.at("area-worktrees/plain");
        std::fs::create_dir_all(&plain).unwrap();
        std::fs::write(plain.join("precious"), "keep\n").unwrap();
        let other = Fixture::new();
        let foreign = other.worktree("area-worktrees/foreign").await;
        let destination = fixture.at("area-worktrees/foreign");
        std::fs::rename(&foreign, &destination).unwrap();

        for standing in [&plain, &destination] {
            let refusal = GitService::new()
                .create_worktree(&fixture.repository, standing, &branch("main"))
                .await;
            assert!(
                matches!(refusal, Err(GitError::UnsafeWorktree(_))),
                "{standing:?}: {refusal:?}"
            );
        }
        assert!(plain.join("precious").exists());
        assert!(destination.join("README").exists());
    }

    /// A registration proves only that a worktree once stood at a path. A
    /// separate clone standing there now is somebody else's, and is kept.
    #[tokio::test]
    async fn a_clone_standing_where_a_worktree_was_registered_is_not_deleted() {
        let fixture = Fixture::new();
        let worktree = fixture.worktree("area-worktrees/one").await;
        std::fs::remove_dir_all(&worktree).unwrap();
        std::fs::create_dir(&worktree).unwrap();
        remote(&worktree);

        assert!(refused(&fixture, &worktree).await);
        assert!(worktree.join("README").exists());
        assert!(worktree.join(".git").is_dir());
    }

    /// Deleting a directory by hand deletes everything inside it, so one
    /// holding another registered worktree -- a locked one, here -- is kept.
    #[tokio::test]
    async fn a_worktree_holding_another_registered_worktree_is_not_deleted_by_hand() {
        let fixture = Fixture::new();
        let outer = fixture.worktree("area-worktrees/outer").await;
        let inner = outer.join("inner");
        GitService::new()
            .create_worktree(&fixture.repository, &inner, &branch("main"))
            .await
            .unwrap();
        git(
            &fixture.repository,
            &["worktree", "lock", inner.to_str().unwrap()],
        );
        std::fs::remove_file(outer.join(".git")).unwrap();

        assert!(refused(&fixture, &outer).await);
        assert!(inner.join("README").exists());
    }

    #[test]
    fn the_managed_worktree_commands_are_hardened_and_use_no_transport() {
        let command = GitService::managed_local(Path::new("/repository"));
        let arguments: Vec<String> = command
            .as_std()
            .get_args()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect();
        let environment: Vec<(String, Option<String>)> = command
            .as_std()
            .get_envs()
            .map(|(key, value)| {
                (
                    key.to_string_lossy().into_owned(),
                    value.map(|value| value.to_string_lossy().into_owned()),
                )
            })
            .collect();

        for pin in [
            "core.hooksPath=/dev/null",
            "core.fsmonitor=false",
            "credential.helper=",
        ] {
            assert!(arguments.contains(&pin.to_string()), "{pin}: {arguments:?}");
        }
        assert!(
            environment.contains(&("GIT_ALLOW_PROTOCOL".to_string(), Some(String::new()))),
            "a local worktree command reaches no transport: {environment:?}"
        );
        assert!(
            environment.contains(&(
                "GIT_CONFIG_GLOBAL".to_string(),
                Some("/dev/null".to_string())
            )),
            "{environment:?}"
        );
    }

    /// A managed clone whose shared configuration a run wrote a key into that no
    /// pin reaches is refused before a worktree of it is added or removed: every
    /// worktree shares that file, so the next worktree command would otherwise
    /// run whatever it names.
    #[tokio::test]
    async fn a_managed_clone_whose_configuration_names_something_no_pin_reaches_is_refused() {
        let fixture = Fixture::new();
        git(&fixture.repository, &["config", "alias.co", "checkout"]);
        let worktree = fixture.at("area-worktrees/one");

        let refusal = GitService::new()
            .create_worktree(&fixture.repository, &worktree, &branch("main"))
            .await;

        assert!(
            matches!(refusal, Err(GitError::UnsafeConfig(ref key)) if key == "alias.co"),
            "{refusal:?}"
        );
        assert!(
            !worktree.exists(),
            "no worktree was added in a refused clone"
        );
        assert!(
            matches!(
                GitService::new()
                    .remove_worktree(&fixture.repository, &worktree)
                    .await,
                Err(GitError::UnsafeConfig(_))
            ),
            "removal is refused in the same clone"
        );
    }
}
