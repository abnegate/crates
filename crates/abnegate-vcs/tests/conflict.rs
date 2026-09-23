//! Reproducing a real merge conflict against a real repository.
//!
//! Every repository here is built from scratch inside a temporary directory, so
//! nothing in these tests can reach the checkout the suite is running from.

use abnegate_vcs::RepositoryUrl;
use abnegate_vcs::conflict::BranchName;
use abnegate_vcs::conflict::CommitSha;
use abnegate_vcs::conflict::Conflict;
use abnegate_vcs::conflict::ConflictError;
use abnegate_vcs::conflict::ConflictRequest;
use abnegate_vcs::conflict::ConflictService;
use abnegate_vcs::conflict::ConflictedPath;
use abnegate_vcs::conflict::has_markers;
use abnegate_vcs::resolution::ResolutionVerdict;
use abnegate_vcs::resolution::judge;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use tempfile::TempDir;

const OURS: &str = "fn value() -> u32 {\n    1\n}\n";
const THEIRS: &str = "fn value() -> u32 {\n    2\n}\n";
const UNRELATED: &str = "# notes\n";

fn git(repository: &Path, arguments: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(repository)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_AUTHOR_NAME", "Fixture")
        .env("GIT_AUTHOR_EMAIL", "fixture@example.test")
        .env("GIT_COMMITTER_NAME", "Fixture")
        .env("GIT_COMMITTER_EMAIL", "fixture@example.test")
        .args(arguments)
        .output()
        .expect("git must be available to run these tests");

    assert!(
        output.status.success(),
        "git {arguments:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn write(repository: &Path, path: &str, content: &str) {
    let target = repository.join(path);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(target, content).unwrap();
}

fn commit(repository: &Path, message: &str) {
    git(repository, &["add", "--all"]);
    git(repository, &["commit", "--quiet", "-m", message]);
}

/// A repository whose `feature` branch conflicts with `main` in `src/value.rs`.
fn conflicting_origin() -> TempDir {
    let origin = TempDir::new().unwrap();
    let path = origin.path();

    git(path, &["init", "--quiet", "--initial-branch", "main"]);
    write(path, "src/value.rs", "fn value() -> u32 {\n    0\n}\n");
    write(path, "README.md", "# project\n");
    commit(path, "initial");

    git(path, &["checkout", "--quiet", "-b", "feature"]);
    write(path, "src/value.rs", THEIRS);
    commit(path, "feature changes the value");

    git(path, &["checkout", "--quiet", "main"]);
    write(path, "src/value.rs", OURS);
    commit(path, "main changes the value too");

    origin
}

/// A repository whose `feature` branch merges cleanly into `main`.
fn clean_origin() -> TempDir {
    let origin = TempDir::new().unwrap();
    let path = origin.path();

    git(path, &["init", "--quiet", "--initial-branch", "main"]);
    write(path, "src/value.rs", "fn value() -> u32 {\n    0\n}\n");
    commit(path, "initial");

    git(path, &["checkout", "--quiet", "-b", "feature"]);
    write(path, "NOTES.md", UNRELATED);
    commit(path, "feature adds notes");

    git(path, &["checkout", "--quiet", "main"]);
    origin
}

/// Resolve the conflict in `src/value.rs` by keeping both sides, and return
/// the conflicted text it replaced.
fn repair(conflict: &Conflict) -> String {
    let file = conflict.confine("src/value.rs").unwrap();
    let conflicted = std::fs::read_to_string(&file).unwrap();
    std::fs::write(
        &file,
        "fn value() -> u32 {\n    1\n}\n\nfn other() -> u32 {\n    2\n}\n",
    )
    .unwrap();
    conflicted
}

/// The repository a checkout's `.git` file points at.
fn repository_of(checkout: &Path) -> PathBuf {
    let pointer = std::fs::read_to_string(checkout.join(".git")).unwrap();
    PathBuf::from(pointer.trim().strip_prefix("gitdir: ").unwrap())
}

fn request(origin: &Path) -> ConflictRequest {
    ConflictRequest {
        remote: RepositoryUrl::local(origin).unwrap(),
        token: None,
        head: BranchName::parse("feature").unwrap(),
        base: BranchName::parse("main").unwrap(),
        expected_head: None,
        expected_base: None,
    }
}

#[tokio::test]
async fn a_conflicting_branch_reproduces_in_a_throwaway_checkout() {
    let origin = conflicting_origin();
    let conflict = ConflictService::new()
        .reproduce(&request(origin.path()))
        .await
        .expect("a branch that conflicts must reproduce");

    assert_eq!(
        conflict.files(),
        &[ConflictedPath::parse("src/value.rs").unwrap()],
        "only the file git reported unmerged belongs to the conflict"
    );

    let checkout = conflict.path().to_path_buf();
    assert!(checkout.is_dir());
    assert!(
        conflict.isolation().is_dir(),
        "a repair is given a home of its own"
    );
    assert_ne!(
        checkout.canonicalize().unwrap(),
        origin.path().canonicalize().unwrap(),
        "the repair must never run in the repository it fetched from"
    );

    let conflicted = std::fs::read_to_string(checkout.join("src/value.rs")).unwrap();
    assert!(
        has_markers(&conflicted),
        "the reproduced checkout must actually hold the conflicted text"
    );
    assert!(conflicted.contains("    1") && conflicted.contains("    2"));

    let isolation = conflict.isolation().to_path_buf();
    drop(conflict);
    assert!(
        !checkout.exists() && !isolation.exists(),
        "the throwaway directory must be gone once the conflict is dropped"
    );
}

#[tokio::test]
async fn a_branch_that_merges_cleanly_is_nothing_to_repair() {
    let origin = clean_origin();
    let outcome = ConflictService::new()
        .reproduce(&request(origin.path()))
        .await;

    assert!(
        matches!(outcome, Err(ConflictError::NoConflict)),
        "a clean merge must report nothing to do rather than invent a repair"
    );
}

#[tokio::test]
async fn a_head_that_moved_since_the_caller_looked_refuses_to_reproduce() {
    let origin = conflicting_origin();
    let mut request = request(origin.path());
    request.expected_head =
        Some(CommitSha::parse("0123456789abcdef0123456789abcdef01234567").unwrap());

    let outcome = ConflictService::new().reproduce(&request).await;
    match outcome {
        Err(ConflictError::Moved { branch, .. }) => assert_eq!(branch.as_str(), "feature"),
        other => panic!("a moved head must refuse the repair, got {other:?}"),
    }
}

#[tokio::test]
async fn the_expected_commits_let_a_caller_pin_the_state_it_reproduced() {
    let origin = conflicting_origin();
    let head = CommitSha::parse(&git(origin.path(), &["rev-parse", "feature"])).unwrap();
    let base = CommitSha::parse(&git(origin.path(), &["rev-parse", "main"])).unwrap();

    let mut request = request(origin.path());
    request.expected_head = Some(head.clone());
    request.expected_base = Some(base.clone());

    let conflict = ConflictService::new().reproduce(&request).await.unwrap();
    assert_eq!(conflict.head(), &head);
    assert_eq!(conflict.base(), &base);
    conflict
        .verify(&ConflictService::new())
        .await
        .expect("a freshly reproduced checkout is at the state it reports");
}

#[tokio::test]
async fn a_checkout_moved_underneath_the_repair_is_refused() {
    let origin = conflicting_origin();
    let service = ConflictService::new();
    let conflict = service.reproduce(&request(origin.path())).await.unwrap();

    git(conflict.path(), &["reset", "--hard", "--quiet"]);
    git(
        conflict.path(),
        &["checkout", "--detach", "--quiet", "refs/conflict/base"],
    );

    assert!(
        matches!(
            conflict.verify(&service).await,
            Err(ConflictError::CheckoutMoved)
        ),
        "a repair applied to a checkout that moved is not a repair of this conflict"
    );
}

#[tokio::test]
async fn only_the_conflicted_files_can_be_reached_from_the_checkout() {
    let origin = conflicting_origin();
    let conflict = ConflictService::new()
        .reproduce(&request(origin.path()))
        .await
        .unwrap();

    let allowed = conflict
        .confine("src/value.rs")
        .expect("a conflicted file is in scope");
    assert_eq!(
        allowed,
        conflict.path().canonicalize().unwrap().join("src/value.rs")
    );

    assert_eq!(
        conflict
            .confine(&allowed.to_string_lossy())
            .expect("the absolute form of a conflicted file is the same file"),
        allowed
    );

    for refused in [
        "README.md",
        "src/../README.md",
        "../outside.rs",
        "/etc/passwd",
        ".git/config",
        "",
    ] {
        assert!(
            conflict.confine(refused).is_err(),
            "{refused:?} is not one of the conflicted files and must be refused"
        );
    }

    let escape = origin.path().join("README.md");
    assert!(
        conflict.confine(&escape.to_string_lossy()).is_err(),
        "an absolute path outside the checkout must be refused"
    );
}

/// The repair a caller applies is only committed once it has kept both
/// branches and touched nothing else, and only the conflicted file and the
/// merge ride along in the commit.
#[tokio::test]
async fn a_repair_that_keeps_both_sides_is_judged_resolved_and_committed_alone() {
    let origin = conflicting_origin();
    let service = ConflictService::new().with_author("Fixture", "fixture@example.test");
    let conflict = service.reproduce(&request(origin.path())).await.unwrap();

    let conflicted = repair(&conflict);
    let repaired = std::fs::read_to_string(conflict.confine("src/value.rs").unwrap()).unwrap();

    assert_eq!(judge(&conflicted, &repaired), ResolutionVerdict::Resolved);
    assert!(
        service.strays(&conflict).await.unwrap().is_empty(),
        "the repair touched nothing the conflict did not name"
    );

    std::fs::write(conflict.path().join("README.md"), "wandered off\n").unwrap();
    assert_eq!(
        service.strays(&conflict).await.unwrap(),
        vec!["README.md".to_string()],
        "a file outside the conflict shows up as a stray"
    );
    assert!(
        matches!(
            service.apply(&conflict, "(fix): merge both sides").await,
            Err(ConflictError::Strays(ref strays)) if strays == &["README.md".to_string()]
        ),
        "a stray refuses the commit rather than being left out of it"
    );

    git(conflict.path(), &["checkout", "--", "README.md"]);
    let committed = service
        .apply(&conflict, "(fix): merge both sides")
        .await
        .unwrap();
    assert_eq!(
        committed.as_str(),
        git(conflict.path(), &["rev-parse", "HEAD"]),
        "the commit reported is the one the checkout is on"
    );
    assert_eq!(
        git(conflict.path(), &["rev-parse", "HEAD^1", "HEAD^2"]),
        format!("{}\n{}", conflict.head(), conflict.base()),
        "the commit is the merge of the two sides"
    );
    assert_eq!(
        git(
            conflict.path(),
            &["diff", "--name-only", conflict.head().as_str(), "HEAD"]
        ),
        "src/value.rs",
        "only the conflicted file differs from the head"
    );
}

/// Taking one branch's side and deleting the other's is exactly the cheap
/// escape the judge exists to catch.
#[tokio::test]
async fn a_repair_that_drops_a_branch_is_judged_a_discard() {
    let origin = conflicting_origin();
    let conflict = ConflictService::new()
        .reproduce(&request(origin.path()))
        .await
        .unwrap();

    let file = conflict.confine("src/value.rs").unwrap();
    let conflicted = std::fs::read_to_string(&file).unwrap();

    assert!(matches!(
        judge(&conflicted, OURS),
        ResolutionVerdict::Discarded(_)
    ));
    assert!(matches!(
        judge(&conflicted, THEIRS),
        ResolutionVerdict::Discarded(_)
    ));
    assert_eq!(
        judge(&conflicted, &conflicted),
        ResolutionVerdict::MarkersRemain
    );
}

#[tokio::test]
async fn a_file_the_repair_created_is_a_stray_that_refuses_the_commit() {
    let origin = conflicting_origin();
    let service = ConflictService::new();
    let conflict = service.reproduce(&request(origin.path())).await.unwrap();
    repair(&conflict);

    write(conflict.path(), "src/extra.rs", "fn extra() {}\n");

    assert_eq!(
        service.strays(&conflict).await.unwrap(),
        vec!["src/extra.rs".to_string()]
    );
    assert!(matches!(
        service.apply(&conflict, "(fix): merge").await,
        Err(ConflictError::Strays(_))
    ));
}

/// A change the repair staged itself matches the index, so it never showed
/// in the working tree's diff, and the commit took the whole index with it.
#[tokio::test]
async fn a_change_the_repair_staged_itself_is_a_stray_that_refuses_the_commit() {
    let origin = conflicting_origin();
    let service = ConflictService::new();
    let conflict = service.reproduce(&request(origin.path())).await.unwrap();
    repair(&conflict);

    write(conflict.path(), "README.md", "# rewritten\n");
    git(conflict.path(), &["add", "README.md"]);

    assert_eq!(
        service.strays(&conflict).await.unwrap(),
        vec!["README.md".to_string()]
    );
    assert!(matches!(
        service.apply(&conflict, "(fix): merge").await,
        Err(ConflictError::Strays(_))
    ));
    assert_eq!(
        git(conflict.path(), &["rev-parse", "HEAD"]),
        conflict.head().as_str(),
        "nothing was committed"
    );
}

/// With the merge no longer in progress, a commit would record the base's
/// changes as the head's own work and drop the base as a parent.
#[tokio::test]
async fn a_merge_no_longer_in_progress_is_refused() {
    let origin = conflicting_origin();
    let service = ConflictService::new();
    let conflict = service.reproduce(&request(origin.path())).await.unwrap();
    repair(&conflict);

    std::fs::remove_file(repository_of(conflict.path()).join("MERGE_HEAD")).unwrap();

    assert!(matches!(
        conflict.verify(&service).await,
        Err(ConflictError::CheckoutMoved)
    ));
    assert!(matches!(
        service.apply(&conflict, "(fix): merge").await,
        Err(ConflictError::CheckoutMoved)
    ));
}

/// The repository lives outside the checkout, so a `.git` the repair plants
/// there -- a configuration naming a monitor and a hooks directory with a
/// hook in it -- is never read by the commands that check and commit it.
#[cfg(unix)]
#[tokio::test]
async fn nothing_planted_in_the_checkout_runs_when_the_repair_is_committed() {
    use std::os::unix::fs::PermissionsExt;

    let origin = conflicting_origin();
    let service = ConflictService::new();
    let conflict = service.reproduce(&request(origin.path())).await.unwrap();
    repair(&conflict);
    let markers = TempDir::new().unwrap();
    let marker = markers.path().join("ran");
    let script = markers.path().join("planted");
    std::fs::write(
        &script,
        format!("#!/bin/sh\ntouch '{}'\n", marker.display()),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();

    let planted = conflict.path().join(".git");
    std::fs::remove_file(&planted).unwrap();
    std::fs::create_dir_all(planted.join("hooks")).unwrap();
    std::fs::copy(&script, planted.join("hooks").join("post-commit")).unwrap();
    std::fs::write(
        planted.join("config"),
        format!(
            "[core]\n\tfsmonitor = {0}\n\thooksPath = hooks\n[filter \"planted\"]\n\tclean = {0}\n",
            script.display()
        ),
    )
    .unwrap();

    service.apply(&conflict, "(fix): merge").await.unwrap();

    assert!(!marker.exists(), "a program planted in the checkout ran");
}

/// git runs with a home of its own outside the checkout, so an ignore file
/// the repair writes under the checkout cannot hide a file it created.
#[tokio::test]
async fn an_ignore_file_the_repair_writes_cannot_hide_a_stray() {
    let origin = conflicting_origin();
    let service = ConflictService::new();
    let conflict = service.reproduce(&request(origin.path())).await.unwrap();
    repair(&conflict);

    write(
        conflict.path(),
        ".config/git/ignore",
        "hidden.rs\n.config/\n",
    );
    write(conflict.path(), "hidden.rs", "fn hidden() {}\n");

    let strays = service.strays(&conflict).await.unwrap();
    assert!(strays.contains(&"hidden.rs".to_string()), "{strays:?}");
    assert!(
        strays.contains(&".config/git/ignore".to_string()),
        "{strays:?}"
    );
}

/// The paths git reports are taken literally: a name with spaces and glob
/// characters in it is a file to repair, not a pattern to refuse.
#[tokio::test]
async fn a_conflicted_file_named_with_spaces_and_glob_characters_is_repaired_literally() {
    let origin = TempDir::new().unwrap();
    let path = origin.path();
    let name = "src/[ab] value*.rs";
    git(path, &["init", "--quiet", "--initial-branch", "main"]);
    write(path, name, "fn value() -> u32 {\n    0\n}\n");
    write(path, "src/a value.rs", "fn untouched() {}\n");
    commit(path, "initial");
    git(path, &["checkout", "--quiet", "-b", "feature"]);
    write(path, name, THEIRS);
    commit(path, "feature changes the value");
    git(path, &["checkout", "--quiet", "main"]);
    write(path, name, OURS);
    commit(path, "main changes the value too");

    let service = ConflictService::new();
    let conflict = service.reproduce(&request(path)).await.unwrap();
    assert_eq!(conflict.files(), &[ConflictedPath::parse(name).unwrap()]);

    std::fs::write(
        conflict.confine(name).unwrap(),
        "fn value() -> u32 {\n    1\n}\n\nfn other() -> u32 {\n    2\n}\n",
    )
    .unwrap();
    service.apply(&conflict, "(fix): merge").await.unwrap();

    assert_eq!(
        git(
            conflict.path(),
            &["diff", "--name-only", conflict.head().as_str(), "HEAD"]
        ),
        name
    );
}

/// The repair is published to the branch and repository it was reproduced
/// from, as the merge the repair committed, and nothing else.
#[tokio::test]
async fn publishing_pushes_the_applied_merge_to_the_branch_it_was_reproduced_from() {
    let origin = conflicting_origin();
    let main = git(origin.path(), &["rev-parse", "main"]);
    let service = ConflictService::new();
    let conflict = service.reproduce(&request(origin.path())).await.unwrap();

    assert!(
        matches!(
            service.publish(&conflict, None).await,
            Err(ConflictError::NotApplied)
        ),
        "a checkout with no resolution committed has nothing to publish"
    );

    repair(&conflict);
    let committed = service.apply(&conflict, "(fix): merge").await.unwrap();
    let published = service.publish(&conflict, None).await.unwrap();

    assert_eq!(published, committed);
    assert_eq!(
        git(origin.path(), &["rev-parse", "feature"]),
        committed.as_str()
    );
    assert_eq!(git(origin.path(), &["rev-parse", "main"]), main);
    assert_eq!(conflict.head_branch().as_str(), "feature");
}

/// A branch somebody advanced while it was being repaired keeps their
/// commits: the push is refused rather than forced.
#[tokio::test]
async fn publishing_over_a_branch_that_moved_is_rejected() {
    let origin = conflicting_origin();
    let service = ConflictService::new();
    let conflict = service.reproduce(&request(origin.path())).await.unwrap();
    repair(&conflict);
    service.apply(&conflict, "(fix): merge").await.unwrap();

    git(origin.path(), &["checkout", "--quiet", "feature"]);
    write(origin.path(), "NOTES.md", "moved on\n");
    commit(origin.path(), "somebody else's work");
    let moved = git(origin.path(), &["rev-parse", "feature"]);
    git(origin.path(), &["checkout", "--quiet", "main"]);

    assert!(matches!(
        service.publish(&conflict, None).await,
        Err(ConflictError::Rejected)
    ));
    assert_eq!(git(origin.path(), &["rev-parse", "feature"]), moved);
}
