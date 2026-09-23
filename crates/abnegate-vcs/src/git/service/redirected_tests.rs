use super::*;
use crate::git::service::hardened::fixtures::branch;
use crate::worktree::fixtures::git;
use tempfile::TempDir;

/// The ref storage every repository here keeps its refs in, whatever the
/// host's git defaults to, so a ref listing reads the same everywhere.
const FILES: &str = "--ref-format=files";

/// A repository at `root/name` with one commit on `main`.
fn repository(root: &Path, name: &str) -> PathBuf {
    let path = root.join(name);
    std::fs::create_dir(&path).unwrap();
    git(&path, &["init", "-q", "-b", "main", FILES]);
    std::fs::write(path.join("README"), "fixture\n").unwrap();
    git(&path, &["add", "README"]);
    git(&path, &["commit", "-q", "-m", "fixture"]);
    path
}

/// A clone of `source` at `root/name`: a repository of its own that a
/// checkout of `source` must never write.
fn cloned(root: &Path, source: &Path, name: &str) -> PathBuf {
    let path = root.join(name);
    git(
        root,
        &[
            "clone",
            "-q",
            FILES,
            "--",
            source.to_str().unwrap(),
            path.to_str().unwrap(),
        ],
    );
    path
}

/// A worktree of `repository` at `path`, added with `options` ahead of it.
fn worktree(repository: &Path, path: &Path, options: &[&str]) -> PathBuf {
    let mut arguments = vec!["worktree", "add", "-q"];
    arguments.extend_from_slice(options);
    arguments.extend(["--", path.to_str().unwrap()]);
    git(repository, &arguments);
    path.to_path_buf()
}

/// The git directory of the worktree at `path`, as git names it.
fn own(path: &Path) -> PathBuf {
    PathBuf::from(git(
        path,
        &["rev-parse", "--path-format=absolute", "--git-dir"],
    ))
}

/// Every ref of the repository at `path`, with what it names.
fn refs(path: &Path) -> String {
    git(
        path,
        &[
            "for-each-ref",
            "--format=%(refname) %(objectname) %(symref)",
        ],
    )
}

/// The [`GitError`] an error from the blocking worktree module carries.
fn carried(error: &std::io::Error) -> Option<&GitError> {
    error
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<GitError>())
}

/// `content` staged in `path`, into whichever index its `.git` leads git to.
fn staged(path: &Path, content: &str) {
    std::fs::write(path.join("work.txt"), content).unwrap();
    git(path, &["add", "--", "work.txt"]);
}

/// A worktree whose `.git` is a link to another clone's git directory hands
/// every command that other clone's refs, index and configuration.
#[cfg(unix)]
#[tokio::test]
async fn a_worktree_whose_git_marker_is_a_link_to_another_clone_is_refused_a_commit() {
    let root = TempDir::new().unwrap();
    let base = repository(root.path(), "base");
    let other = cloned(root.path(), &base, "other");
    let checkout = worktree(&base, &root.path().join("one"), &["-b", "task/one"]);
    let marker = checkout.join(GIT_DIRECTORY);
    std::fs::remove_file(&marker).unwrap();
    std::os::unix::fs::symlink(other.join(GIT_DIRECTORY), &marker).unwrap();
    staged(&checkout, "work\n");
    let listed = refs(&other);

    let committed = GitService::new().commit(&checkout, "work").await;
    let blocking = crate::worktree::unfinished(&checkout, &[]);

    assert_eq!(
        refs(&other),
        listed,
        "the refused commit moved the other clone's refs"
    );
    assert!(
        matches!(committed, Err(GitError::LinkedPath)),
        "{committed:?}"
    );
    assert!(
        matches!(
            blocking.as_ref().err().and_then(carried),
            Some(GitError::LinkedPath)
        ),
        "{blocking:?}"
    );
}

/// Resetting a clone's configuration replaces `.git/config` by name, and a
/// `.git` that is a link to another clone's git directory, or a file, would
/// have it replace that clone's configuration or write beside a worktree's.
#[cfg(unix)]
#[tokio::test]
async fn a_configuration_reset_writes_no_configuration_but_the_clone_s_own() {
    let url = RepositoryUrl::parse("https://github.com/owner/repository").unwrap();
    let root = TempDir::new().unwrap();
    let base = repository(root.path(), "base");
    let other = cloned(root.path(), &base, "other");
    let linked = root.path().join("linked");
    std::fs::create_dir(&linked).unwrap();
    std::os::unix::fs::symlink(other.join(GIT_DIRECTORY), linked.join(GIT_DIRECTORY)).unwrap();
    let checkout = worktree(&base, &root.path().join("one"), &["--detach"]);
    let configuration =
        |path: &Path| std::fs::read_to_string(path.join(GIT_DIRECTORY).join(CONFIG_FILE));
    let (others, bases) = (
        configuration(&other).unwrap(),
        configuration(&base).unwrap(),
    );
    let service = GitService::new();

    let through_link = service.reset_config(&linked, &url).await;
    let through_file = service.reset_config(&checkout, &url).await;

    assert_eq!(
        configuration(&other).unwrap(),
        others,
        "the other clone's configuration was rewritten"
    );
    assert_eq!(configuration(&base).unwrap(), bases);
    assert!(
        matches!(through_link, Err(GitError::LinkedPath)),
        "{through_link:?}"
    );
    assert!(
        matches!(through_file, Err(GitError::RedirectedGitDirectory)),
        "{through_file:?}"
    );
}

/// A worktree's record names the directory every worktree of its
/// repository shares in `commondir`, and git keeps every ref there.
#[tokio::test]
async fn a_worktree_whose_record_names_another_clone_as_shared_is_refused_a_branch() {
    let root = TempDir::new().unwrap();
    let base = repository(root.path(), "base");
    let other = cloned(root.path(), &base, "other");
    let checkout = worktree(&base, &root.path().join("one"), &["--detach"]);
    std::fs::write(
        own(&checkout).join("commondir"),
        format!("{}\n", other.join(GIT_DIRECTORY).display()),
    )
    .unwrap();
    let listed = refs(&other);

    let created = GitService::new()
        .create_branch(&checkout, &branch("feature/one"))
        .await;
    let blocking = crate::worktree::unfinished(&checkout, &[]);

    assert_eq!(
        refs(&other),
        listed,
        "the refused branch was made in the other clone"
    );
    assert!(
        matches!(created, Err(GitError::RedirectedGitDirectory)),
        "{created:?}"
    );
    assert!(
        matches!(
            blocking.as_ref().err().and_then(carried),
            Some(GitError::RedirectedGitDirectory)
        ),
        "{blocking:?}"
    );
}

/// A worktree's `.git` file names the git directory it works in, and one
/// rewritten to name another clone's, or another clone's record of a
/// worktree of its own, hands every command that clone's refs.
#[tokio::test]
async fn a_worktree_whose_git_file_names_another_git_directory_is_refused_a_commit() {
    for record in [false, true] {
        let root = TempDir::new().unwrap();
        let base = repository(root.path(), "base");
        let other = cloned(root.path(), &base, "other");
        let named = match record {
            false => other.join(GIT_DIRECTORY),
            true => own(&worktree(
                &other,
                &root.path().join("two"),
                &["-b", "task/two"],
            )),
        };
        let checkout = worktree(&base, &root.path().join("one"), &["-b", "task/one"]);
        std::fs::write(
            checkout.join(GIT_DIRECTORY),
            format!("gitdir: {}\n", named.display()),
        )
        .unwrap();
        staged(&checkout, "work\n");
        let listed = refs(&other);

        let committed = GitService::new().commit(&checkout, "work").await;
        let blocking = crate::worktree::unfinished(&checkout, &[]);

        assert_eq!(
            refs(&other),
            listed,
            "record {record}: the refused commit moved the other clone's refs"
        );
        assert!(
            matches!(committed, Err(GitError::RedirectedGitDirectory)),
            "record {record}: {committed:?}"
        );
        assert!(
            matches!(
                blocking.as_ref().err().and_then(carried),
                Some(GitError::RedirectedGitDirectory)
            ),
            "record {record}: {blocking:?}"
        );
    }
}

/// A worktree whose `.git` is gone is no checkout at all, and git looks for
/// one in every directory above it: a repository enclosing the directory
/// the worktrees are kept in would take every write meant for it.
#[tokio::test]
async fn a_worktree_whose_git_file_is_gone_is_refused_before_an_enclosing_repository_is_written() {
    let root = TempDir::new().unwrap();
    let enclosing = repository(root.path(), "enclosing");
    let base = cloned(root.path(), &enclosing, "base");
    let checkout = worktree(&base, &enclosing.join("one"), &["-b", "task/one"]);
    std::fs::remove_file(checkout.join(GIT_DIRECTORY)).unwrap();
    std::fs::write(checkout.join("work.txt"), "work\n").unwrap();
    let service = GitService::new();
    let listed = refs(&enclosing);
    let tracked = git(&enclosing, &["ls-files"]);

    let staged = service.stage_all(&checkout).await;
    let committed = service.commit(&checkout, "work").await;
    let blocking = crate::worktree::unfinished(&checkout, &[]);

    assert_eq!(
        git(&enclosing, &["ls-files"]),
        tracked,
        "the refused staging wrote the enclosing repository's index"
    );
    assert_eq!(
        refs(&enclosing),
        listed,
        "the refused commit moved the enclosing repository's refs"
    );
    for (operation, refusal) in [staged, committed.map(drop)].into_iter().enumerate() {
        assert!(
            matches!(refusal, Err(GitError::RedirectedGitDirectory)),
            "operation {operation}: {refusal:?}"
        );
    }
    assert!(
        matches!(
            blocking.as_ref().err().and_then(carried),
            Some(GitError::RedirectedGitDirectory)
        ),
        "{blocking:?}"
    );
}

/// A clone, a worktree git records by absolute paths, one it records by
/// relative ones, and one the blocking module added all work as before.
#[tokio::test]
async fn a_clone_and_every_worktree_git_adds_of_it_are_accepted() {
    let root = TempDir::new().unwrap();
    let base = repository(root.path(), "base");
    let service = GitService::new();
    let absolute = worktree(&base, &root.path().join("absolute"), &["-b", "task/one"]);
    git(
        &base,
        &[
            "-c",
            "worktree.useRelativePaths=true",
            "worktree",
            "add",
            "-q",
            "-b",
            "task/two",
            "--",
            root.path().join("relative").to_str().unwrap(),
        ],
    );
    let relative = root.path().join("relative");
    let added = root.path().join("added");
    crate::worktree::add(&base, &added, "HEAD").unwrap();

    service
        .create_branch(&base, &branch("feature/base"))
        .await
        .unwrap();
    for checkout in [&absolute, &relative, &added] {
        staged(checkout, "work\n");
        service.stage_all(checkout).await.unwrap();
        let committed = service.commit(checkout, "work").await.unwrap();
        assert_eq!(
            service.revision(checkout, "HEAD").await.unwrap(),
            committed,
            "{}",
            checkout.display()
        );
        assert!(
            !crate::worktree::unfinished(checkout, &[committed.as_str()])
                .unwrap()
                .any(),
            "{}",
            checkout.display()
        );
    }
    assert_eq!(
        crate::worktree::repository_of(&relative).unwrap(),
        base.canonicalize().unwrap()
    );
}

/// A checkout named through a linked directory above it -- `/tmp` on
/// macOS is one -- is its own, since both sides are compared by their real
/// paths.
#[cfg(unix)]
#[tokio::test]
async fn a_checkout_named_through_a_linked_parent_directory_is_accepted() {
    let root = TempDir::new().unwrap();
    let real = root.path().join("real");
    std::fs::create_dir(&real).unwrap();
    let alias = root.path().join("alias");
    std::os::unix::fs::symlink(&real, &alias).unwrap();
    let base = repository(&alias, "base");
    let checkout = worktree(&base, &alias.join("one"), &["-b", "task/one"]);
    let service = GitService::new();

    for (clone, linked, content) in [
        (base.clone(), checkout.clone(), "through the link\n"),
        (real.join("base"), real.join("one"), "by the real path\n"),
    ] {
        assert_eq!(service.current_branch(&clone).await.unwrap(), "main");
        staged(&linked, content);
        service.stage_all(&linked).await.unwrap();
        service.commit(&linked, "work").await.unwrap();
        assert!(
            crate::worktree::unfinished(&linked, &[]).is_ok(),
            "{}",
            linked.display()
        );
    }
}

/// Git reads objects from every store `objects/info/alternates` names, and
/// a push uploads whatever HEAD reaches from any of them, so a repository
/// that names one is refused, however empty the file, whether it is worked
/// in as a clone or through a worktree of it; a clone naming none is not.
#[tokio::test]
async fn a_repository_that_borrows_objects_from_another_store_is_refused() {
    let root = TempDir::new().unwrap();
    let source = repository(root.path(), "source");
    let base = cloned(root.path(), &source, "base");
    let checkout = worktree(&base, &root.path().join("one"), &["-b", "task/one"]);
    let service = GitService::new();
    staged(&base, "ordinary\n");
    service.commit(&base, "ordinary").await.unwrap();
    std::fs::write(
        base.join(GIT_DIRECTORY)
            .join("objects")
            .join("info")
            .join("alternates"),
        "",
    )
    .unwrap();
    staged(&base, "borrowing\n");
    staged(&checkout, "borrowing\n");
    let listed = refs(&base);

    let committed = service.commit(&base, "borrowing").await;
    let linked = service.commit(&checkout, "borrowing").await;
    let blocking = crate::worktree::unfinished(&checkout, &[]);

    assert_eq!(refs(&base), listed, "the refused commit moved a ref");
    for (operation, refusal) in [committed, linked].into_iter().enumerate() {
        assert!(
            matches!(refusal, Err(GitError::AlternateObjects)),
            "operation {operation}: {refusal:?}"
        );
    }
    assert!(
        matches!(
            blocking.as_ref().err().and_then(carried),
            Some(GitError::AlternateObjects)
        ),
        "{blocking:?}"
    );
    assert_eq!(
        GitError::AlternateObjects.to_string(),
        "Refusing a repository that borrows objects from another store"
    );
}
