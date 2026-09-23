use super::*;
use crate::git::service::hardened::fixtures::branch;
use crate::worktree::fixtures::git;
use std::os::unix::fs::MetadataExt;
use tempfile::TempDir;

/// The ref storage a repository here keeps its refs in unless a test names
/// another: each puts a link where the files layout keeps a file or a
/// directory, whatever the host's git defaults to.
const FILES: &str = "--ref-format=files";

/// A repository at `root/name` with one commit on `main`, its refs stored
/// as `format` names.
fn repository(root: &Path, name: &str, format: &str) -> PathBuf {
    let path = root.join(name);
    std::fs::create_dir(&path).unwrap();
    git(&path, &["init", "-q", "-b", "main", format]);
    std::fs::write(path.join("README"), "fixture\n").unwrap();
    git(&path, &["add", "README"]);
    git(&path, &["commit", "-q", "-m", "fixture"]);
    path
}

/// A clone of `address` at `root/cloned`, its refs stored as files.
fn cloned(root: &Path, address: &str) -> PathBuf {
    let path = root.join("cloned");
    git(
        root,
        &["clone", "-q", FILES, "--", address, path.to_str().unwrap()],
    );
    path
}

/// Move what stands at `standing` to `moved`, and leave a symbolic link to
/// it in its place.
fn relink(standing: &Path, moved: &Path) {
    std::fs::rename(standing, moved).unwrap();
    std::os::unix::fs::symlink(moved, standing).unwrap();
}

/// Every file and directory at or below `path`, as the file it is and what
/// each file holds, read without following a link.
fn snapshot(path: &Path) -> Vec<(PathBuf, u64, Option<Vec<u8>>)> {
    let mut found = Vec::new();
    let mut pending = vec![path.to_path_buf()];
    while let Some(next) = pending.pop() {
        let details = std::fs::symlink_metadata(&next).unwrap();
        let held = match details.is_dir() {
            true => {
                pending.extend(
                    std::fs::read_dir(&next)
                        .unwrap()
                        .map(|entry| entry.unwrap().path()),
                );
                None
            }
            false => Some(std::fs::read(&next).unwrap()),
        };
        found.push((next, details.ino(), held));
    }
    found.sort();
    found
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

/// A linked worktree keeps its own `HEAD`'s reflog in its own git
/// directory, and a commit appends to it through a link standing there.
#[tokio::test]
async fn a_worktree_whose_own_reflogs_are_a_link_is_refused_a_commit() {
    let root = TempDir::new().unwrap();
    let base = repository(root.path(), "base", FILES);
    let worktree = root.path().join("one");
    git(
        &base,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "task/one",
            worktree.to_str().unwrap(),
        ],
    );
    std::fs::write(worktree.join("work.txt"), "work\n").unwrap();
    let service = GitService::new();
    let checkout = Checkout::linked(&worktree, &base);
    service.stage_all(&checkout).await.unwrap();
    let own = PathBuf::from(git(
        &worktree,
        &["rev-parse", "--path-format=absolute", "--git-dir"],
    ));
    let moved = root.path().join("moved");
    relink(&own.join("logs"), &moved);
    let (before, listed) = (snapshot(&moved), refs(&base));

    let committed = service.commit(&checkout, "work").await;

    assert_eq!(
        snapshot(&moved),
        before,
        "the commit wrote through the link"
    );
    assert_eq!(refs(&base), listed, "the refused commit moved a ref");
    assert!(
        matches!(committed, Err(GitError::LinkedPath)),
        "{committed:?}"
    );
}

/// A clone keeps `HEAD`'s reflog in `logs/HEAD`, and bringing the clone
/// forward appends to it through a link standing there, though no command
/// starts a reflog.
#[tokio::test]
async fn a_clone_whose_head_reflog_is_a_link_is_refused_a_sync() {
    let root = TempDir::new().unwrap();
    let source = repository(root.path(), "source", FILES);
    let url = format!("file://{}", source.display());
    let clone = cloned(root.path(), &url);
    let moved = root.path().join("moved");
    relink(&clone.join(GIT_DIRECTORY).join("logs/HEAD"), &moved);
    let (before, listed) = (snapshot(&moved), refs(&clone));
    git(&source, &["commit", "-q", "--allow-empty", "-m", "advance"]);

    let synced = GitService::new().ensure_synced(&clone, &url).await;

    assert_eq!(
        snapshot(&moved),
        before,
        "bringing the clone forward wrote through the link"
    );
    assert_eq!(refs(&clone), listed, "the refused sync moved a ref");
    assert!(matches!(synced, Err(GitError::LinkedPath)), "{synced:?}");
}

/// A repository whose refs are kept in a reftable writes every change of
/// them as a new table in `reftable/`, wherever a link standing there
/// points.
#[tokio::test]
async fn a_repository_whose_ref_tables_are_a_link_is_refused_a_new_branch() {
    let root = TempDir::new().unwrap();
    let repository = repository(root.path(), "repository", "--ref-format=reftable");
    let moved = root.path().join("moved");
    relink(&repository.join(GIT_DIRECTORY).join("reftable"), &moved);
    let (before, listed) = (snapshot(&moved), refs(&repository));

    let created = GitService::new()
        .create_branch(&Checkout::base(&repository), &branch("feature/one"))
        .await;

    assert_eq!(
        snapshot(&moved),
        before,
        "the new branch was written through the link"
    );
    assert_eq!(refs(&repository), listed, "the refused branch was made");
    assert!(matches!(created, Err(GitError::LinkedPath)), "{created:?}");
}

/// Git keeps its records of a repository's worktrees in `worktrees/`, and
/// preparing a branch prunes every record there that names no worktree,
/// deleting whatever the directory a link standing there points at holds.
#[tokio::test]
async fn a_repository_whose_worktree_records_are_a_link_is_refused_a_branch() {
    let root = TempDir::new().unwrap();
    let repository = repository(root.path(), "repository", FILES);
    git(&repository, &["branch", "task/one"]);
    let records = root.path().join("records");
    std::fs::create_dir(&records).unwrap();
    std::fs::write(records.join("kept"), "kept\n").unwrap();
    std::os::unix::fs::symlink(&records, repository.join(GIT_DIRECTORY).join("worktrees")).unwrap();
    let (before, listed) = (snapshot(&records), refs(&repository));

    let prepared = GitService::new()
        .prepare_branch(&Checkout::base(&repository), &branch("task/one"), false)
        .await;

    assert_eq!(
        snapshot(&records),
        before,
        "preparing the branch deleted through the link"
    );
    assert_eq!(refs(&repository), listed, "the refused branch was moved");
    assert!(
        matches!(prepared, Err(GitError::LinkedPath)),
        "{prepared:?}"
    );
}

/// A commit rewrites the index it was made from, and git renames the new
/// one over whatever file a link standing in its place points at.
#[tokio::test]
async fn a_repository_whose_index_is_a_link_is_refused_a_commit() {
    let root = TempDir::new().unwrap();
    let repository = repository(root.path(), "repository", FILES);
    std::fs::write(repository.join("work.txt"), "work\n").unwrap();
    let service = GitService::new();
    let checkout = Checkout::base(&repository);
    service.stage_all(&checkout).await.unwrap();
    let moved = root.path().join("moved");
    relink(&repository.join(GIT_DIRECTORY).join("index"), &moved);
    let (before, listed) = (snapshot(&moved), refs(&repository));

    let committed = service.commit(&checkout, "work").await;

    assert_eq!(
        snapshot(&moved),
        before,
        "the commit rewrote the index through the link"
    );
    assert_eq!(refs(&repository), listed, "the refused commit moved a ref");
    assert!(
        matches!(committed, Err(GitError::LinkedPath)),
        "{committed:?}"
    );
}

/// `commit -m` writes its message to `COMMIT_EDITMSG` before anything
/// else, truncating whatever file a link standing there points at.
#[tokio::test]
async fn a_repository_whose_commit_message_file_is_a_link_is_refused_a_commit() {
    let root = TempDir::new().unwrap();
    let repository = repository(root.path(), "repository", FILES);
    std::fs::write(repository.join("work.txt"), "work\n").unwrap();
    let service = GitService::new();
    let checkout = Checkout::base(&repository);
    service.stage_all(&checkout).await.unwrap();
    let moved = root.path().join("moved");
    relink(
        &repository.join(GIT_DIRECTORY).join("COMMIT_EDITMSG"),
        &moved,
    );
    let (before, listed) = (snapshot(&moved), refs(&repository));

    let committed = service.commit(&checkout, "work").await;

    assert_eq!(
        snapshot(&moved),
        before,
        "the commit wrote its message through the link"
    );
    assert_eq!(refs(&repository), listed, "the refused commit moved a ref");
    assert!(
        matches!(committed, Err(GitError::LinkedPath)),
        "{committed:?}"
    );
}
