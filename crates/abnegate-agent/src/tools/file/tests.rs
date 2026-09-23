use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use tempfile::tempdir;

use super::list::LIST_FILES_CAP;
use super::patch::ApplyPatchParameters;
use super::read::{FILE_PAGE_CHARACTERS, page_text};
use super::search::{SEARCH_MAX_RESULTS, search_directory};
use super::write::WriteFileParameters;
use super::{ApplyPatchTool, ListFilesTool, ReadFileTool, SearchCodeTool, WriteFileTool};
use crate::test_support::captured_logs;
use crate::tools::{DEFAULT_APPLICATION, Session, Tier, Tool, ToolContext};

const ATTEMPTS: usize = 2_000;
/// Roughly the gap between a tool's check and the open that follows it, so
/// a swap lands inside that gap often rather than once in a long while.
const HELD: Duration = Duration::from_micros(2);
const KEEP: &str = "keep";
const LEAF: &str = "leaf.rs";
const SECRET_FILE: &str = "secret.rs";
const INSIDE: &str = "written inside cwd";
const SECRET: &str = "swordfish";

/// Canonical, so a `cwd` under a symlinked temporary directory (`/var` is
/// `/private/var` on macOS) compares equal to the paths resolved inside it.
fn create_test_context(directory: &Path) -> ToolContext {
    let cwd = directory
        .canonicalize()
        .unwrap_or_else(|_| directory.to_path_buf());
    ToolContext {
        working_directory: cwd,
        env: std::collections::HashMap::new(),
        max_file_size: 1024 * 1024,
        command_timeout: std::time::Duration::from_secs(30),
        unrestricted: false,
        session: Session::Detached,
        application: DEFAULT_APPLICATION.to_string(),
    }
}

#[test]
fn test_read_file_tool_metadata() {
    let tool = ReadFileTool;
    assert_eq!(tool.name(), "read_file");
    assert!(!tool.description().is_empty());

    let schema = tool.parameters_schema();
    assert!(schema.get("properties").is_some());
    assert!(schema.get("required").is_some());
}

#[tokio::test]
async fn test_read_file_success() {
    let directory = tempdir().unwrap();
    let file_path = directory.path().join("test.txt");
    fs::write(&file_path, "Hello, World!\nLine 2\nLine 3").unwrap();

    let tool = ReadFileTool;
    let context = create_test_context(directory.path());

    let result = tool
        .execute(serde_json::json!({"path": "test.txt"}), &context)
        .await
        .unwrap();

    assert!(result.success);
    assert!(result.output.unwrap().contains("Hello, World!"));
}

#[tokio::test]
async fn test_read_file_with_line_range() {
    let directory = tempdir().unwrap();
    let file_path = directory.path().join("test.txt");
    fs::write(&file_path, "Line 1\nLine 2\nLine 3\nLine 4").unwrap();

    let tool = ReadFileTool;
    let context = create_test_context(directory.path());

    let result = tool
        .execute(
            serde_json::json!({"path": "test.txt", "start_line": 2, "end_line": 3}),
            &context,
        )
        .await
        .unwrap();

    assert!(result.success);
    let output = result.output.unwrap();
    assert!(output.contains("Line 2"));
    assert!(output.contains("Line 3"));
    assert!(!output.contains("Line 1"));
    assert!(!output.contains("Line 4"));
}

#[test]
fn page_text_caps_and_continues_by_character() {
    let content = "α".repeat(10);
    let (page, total, next) = page_text(&content, 0, 4).unwrap();
    assert_eq!(page, "αααα");
    assert_eq!(total, 10);
    assert_eq!(next, Some(4));
    let (rest, _, next) = page_text(&content, 4, FILE_PAGE_CHARACTERS).unwrap();
    assert_eq!(rest, "α".repeat(6));
    assert_eq!(next, None);
    assert!(page_text(&content, 0, 0).is_err());
    assert!(page_text(&content, 11, 1).is_err());
}

#[tokio::test]
async fn read_file_pages_large_content_and_continues() {
    let directory = tempdir().unwrap();
    let total = FILE_PAGE_CHARACTERS + 123;
    let content = "α".repeat(total);
    fs::write(directory.path().join("large.txt"), &content).unwrap();

    let tool = ReadFileTool;
    let context = create_test_context(directory.path());

    let result = tool
        .execute(
            serde_json::json!({"path": "large.txt", "limit": 1_000_000}),
            &context,
        )
        .await
        .unwrap();

    assert!(result.success);
    let output = result.output.unwrap();
    let (page, footer) = output.rsplit_once('\n').expect("truncation footer");
    assert_eq!(page.chars().count(), FILE_PAGE_CHARACTERS);
    assert_eq!(
        footer,
        format!("[truncated; total={total} offset=0 next={FILE_PAGE_CHARACTERS}]")
    );

    let continued = tool
        .execute(
            serde_json::json!({"path": "large.txt", "offset": FILE_PAGE_CHARACTERS}),
            &context,
        )
        .await
        .unwrap();
    let rest = continued.output.unwrap();
    assert!(!rest.contains("[truncated;"));
    assert_eq!(rest, "α".repeat(123));
}

#[tokio::test]
async fn read_file_rejects_invalid_page() {
    let directory = tempdir().unwrap();
    fs::write(directory.path().join("short.txt"), "hello").unwrap();
    let tool = ReadFileTool;
    let context = create_test_context(directory.path());

    let limit_zero = tool
        .execute(
            serde_json::json!({"path": "short.txt", "limit": 0}),
            &context,
        )
        .await;
    assert!(limit_zero.unwrap_err().to_string().contains("invalid"));

    let past_end = tool
        .execute(
            serde_json::json!({"path": "short.txt", "offset": 6}),
            &context,
        )
        .await;
    assert!(past_end.unwrap_err().to_string().contains("invalid"));
}

#[tokio::test]
async fn test_read_file_not_found() {
    let directory = tempdir().unwrap();
    let tool = ReadFileTool;
    let context = create_test_context(directory.path());

    let result = tool
        .execute(serde_json::json!({"path": "nonexistent.txt"}), &context)
        .await;

    assert!(result.is_err());
}

#[test]
fn test_write_file_tool_metadata() {
    let tool = WriteFileTool;
    assert_eq!(tool.name(), "write_file");
    assert!(!tool.description().is_empty());
}

#[tokio::test]
async fn test_write_file_success() {
    let directory = tempdir().unwrap();
    let tool = WriteFileTool;
    let context = create_test_context(directory.path());

    let result = tool
        .execute(
            serde_json::json!({"path": "output.txt", "content": "Test content"}),
            &context,
        )
        .await
        .unwrap();

    assert!(result.success);

    let content = fs::read_to_string(directory.path().join("output.txt")).unwrap();
    assert_eq!(content, "Test content");
}

#[tokio::test]
async fn test_write_file_append() {
    let directory = tempdir().unwrap();
    let file_path = directory.path().join("output.txt");
    fs::write(&file_path, "Initial\n").unwrap();

    let tool = WriteFileTool;
    let context = create_test_context(directory.path());

    let result = tool
        .execute(
            serde_json::json!({"path": "output.txt", "content": "Appended", "append": true}),
            &context,
        )
        .await
        .unwrap();

    assert!(result.success);

    let content = fs::read_to_string(&file_path).unwrap();
    assert!(content.contains("Initial"));
    assert!(content.contains("Appended"));
}

#[tokio::test]
async fn test_write_file_creates_dirs() {
    let directory = tempdir().unwrap();
    let tool = WriteFileTool;
    let context = create_test_context(directory.path());

    let result = tool
        .execute(
            serde_json::json!({"path": "subdir/nested/file.txt", "content": "Nested content"}),
            &context,
        )
        .await
        .unwrap();

    assert!(result.success);
    assert!(directory.path().join("subdir/nested/file.txt").exists());
}

/// A path with no `..` and no leading `/` passes the string check, so the
/// canonical check is the only thing standing between a symlink and an
/// escape. Both of `write_file`'s canonical checks were removable with the
/// whole suite green, and every `..` test tripped the string check first.
#[cfg(unix)]
fn symlinked(inside: &Path, name: &str, target: &Path) {
    std::os::unix::fs::symlink(target, inside.join(name)).expect("a symlink");
}

#[cfg(unix)]
#[tokio::test]
async fn read_file_refuses_a_symlink_that_leaves_cwd() {
    let outside = tempdir().unwrap();
    let secret = outside.path().join("id_rsa");
    fs::write(&secret, "PRIVATE KEY BODY").unwrap();
    let inside = tempdir().unwrap();
    symlinked(inside.path(), "notes.txt", &secret);
    let context = create_test_context(inside.path());

    let error = ReadFileTool
        .execute(serde_json::json!({"path": "notes.txt"}), &context)
        .await
        .expect_err("a symlink out of cwd must be refused");

    assert!(
        error.to_string().contains("escapes working directory"),
        "{error}"
    );
}

#[tokio::test]
async fn read_file_refuses_an_absolute_path_outside_cwd() {
    let outside = tempdir().unwrap();
    let secret = outside.path().join("id_rsa");
    fs::write(&secret, "PRIVATE KEY BODY").unwrap();
    let inside = tempdir().unwrap();
    let context = create_test_context(inside.path());

    for path in [secret.to_str().unwrap(), "../../../../../../etc/passwd"] {
        let error = ReadFileTool
            .execute(serde_json::json!({"path": path}), &context)
            .await
            .expect_err("a path outside cwd must be refused");
        assert!(
            error.to_string().contains("escapes working directory"),
            "{path}: {error}"
        );
    }
}

/// Every other file tool canonicalizes `cwd` before comparing; `read_file`
/// compared against it raw, so a caller whose `cwd` merely contains a
/// symlink was refused its own files. `create_test_context` canonicalizes,
/// which hid it.
#[cfg(unix)]
#[tokio::test]
async fn read_file_accepts_its_own_file_under_a_symlinked_cwd() {
    let root = tempdir().unwrap();
    let real = root.path().join("real");
    fs::create_dir(&real).unwrap();
    fs::write(real.join("inside.txt"), "legitimate content").unwrap();
    symlinked(root.path(), "link", &real);

    let mut context = create_test_context(root.path());
    context.working_directory = root.path().join("link");

    let result = ReadFileTool
        .execute(serde_json::json!({"path": "inside.txt"}), &context)
        .await
        .expect("a file inside cwd must be readable");

    assert!(result.output.unwrap().contains("legitimate content"));
}

#[cfg(unix)]
#[tokio::test]
async fn write_file_refuses_a_symlinked_directory_that_leaves_cwd() {
    let outside = tempdir().unwrap();
    let inside = tempdir().unwrap();
    symlinked(inside.path(), "escape", outside.path());
    let context = create_test_context(inside.path());

    let error = WriteFileTool
        .execute(
            serde_json::json!({"path": "escape/pwned.txt", "content": "malicious"}),
            &context,
        )
        .await
        .expect_err("a write through a symlinked directory must be refused");

    assert!(
        error.to_string().contains("escapes working directory"),
        "{error}"
    );
    assert!(
        !outside.path().join("pwned.txt").exists(),
        "the file was written outside cwd"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn write_file_creates_no_directory_outside_cwd_before_refusing() {
    let outside = tempdir().unwrap();
    let inside = tempdir().unwrap();
    symlinked(inside.path(), "escape", outside.path());
    let context = create_test_context(inside.path());

    let error = WriteFileTool
        .execute(
            serde_json::json!({"path": "escape/made/up/pwned.txt", "content": "malicious"}),
            &context,
        )
        .await
        .expect_err("a write through a symlinked directory must be refused");

    assert!(
        error.to_string().contains("escapes working directory"),
        "{error}"
    );
    assert!(
        !outside.path().join("made").exists(),
        "a directory was created outside cwd on the way to refusing the write"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn write_file_refuses_a_symlinked_file_that_leaves_cwd() {
    let outside = tempdir().unwrap();
    let target = outside.path().join("authorized_keys");
    fs::write(&target, "original").unwrap();
    let inside = tempdir().unwrap();
    symlinked(inside.path(), "notes.txt", &target);
    let context = create_test_context(inside.path());

    let error = WriteFileTool
        .execute(
            serde_json::json!({"path": "notes.txt", "content": "malicious"}),
            &context,
        )
        .await
        .expect_err("a write through a symlinked file must be refused");

    assert!(
        error.to_string().contains("escapes working directory"),
        "{error}"
    );
    assert_eq!(fs::read_to_string(&target).unwrap(), "original");
}

#[cfg(unix)]
#[tokio::test]
async fn apply_patch_refuses_a_symlink_that_leaves_cwd() {
    let outside = tempdir().unwrap();
    let target = outside.path().join("authorized_keys");
    fs::write(&target, "original\n").unwrap();
    let inside = tempdir().unwrap();
    symlinked(inside.path(), "notes.txt", &target);
    let context = create_test_context(inside.path());

    let error = ApplyPatchTool
        .execute(
            serde_json::json!({
                "path": "notes.txt",
                "old_string": "original",
                "new_string": "malicious",
            }),
            &context,
        )
        .await
        .expect_err("a patch through a symlink out of cwd must be refused");

    assert!(
        error.to_string().contains("escapes working directory"),
        "{error}"
    );
    assert_eq!(fs::read_to_string(&target).unwrap(), "original\n");
}

#[tokio::test]
async fn list_files_refuses_a_path_outside_cwd() {
    let outside = tempdir().unwrap();
    fs::write(outside.path().join("id_rsa"), "PRIVATE").unwrap();
    let inside = tempdir().unwrap();
    let context = create_test_context(inside.path());

    for path in [outside.path().to_str().unwrap(), "../../../../../../etc"] {
        let error = ListFilesTool
            .execute(serde_json::json!({"path": path}), &context)
            .await
            .expect_err("a directory outside cwd must not be listed");
        assert!(
            error.to_string().contains("escapes working directory"),
            "{path}: {error}"
        );
    }
}

#[cfg(unix)]
#[tokio::test]
async fn list_files_does_not_follow_a_symlink_out_of_cwd() {
    let outside = tempdir().unwrap();
    fs::create_dir(outside.path().join("private")).unwrap();
    fs::write(outside.path().join("private/id_rsa"), "PRIVATE").unwrap();
    let inside = tempdir().unwrap();
    fs::write(inside.path().join("own.txt"), "mine").unwrap();
    symlinked(inside.path(), "hop", outside.path());
    let context = create_test_context(inside.path());

    let result = ListFilesTool
        .execute(
            serde_json::json!({"path": ".", "recursive": true}),
            &context,
        )
        .await
        .expect("listing cwd");

    let output = result.output.unwrap();
    assert!(output.contains("own.txt"), "{output}");
    assert!(!output.contains("id_rsa"), "the walk left cwd: {output}");
}

#[tokio::test]
async fn search_code_refuses_a_path_outside_cwd() {
    let outside = tempdir().unwrap();
    fs::write(
        outside.path().join("secrets.env"),
        "DEPLOY_PHRASE=open sesame please\n",
    )
    .unwrap();
    let inside = tempdir().unwrap();
    let context = create_test_context(inside.path());

    for path in [outside.path().to_str().unwrap(), "../../../../../../etc"] {
        let error = SearchCodeTool
            .execute(
                serde_json::json!({"pattern": "open sesame please", "path": path}),
                &context,
            )
            .await
            .expect_err("a directory outside cwd must not be searched");
        assert!(
            error.to_string().contains("escapes working directory"),
            "{path}: {error}"
        );
    }
}

/// Driven against the walker rather than the tool: ripgrep does not follow
/// symlinks without `-L`, so a tool-level assertion would hold with the
/// guard gone on any host where `rg` is installed.
#[cfg(unix)]
#[test]
fn the_search_walk_does_not_follow_a_symlink_out_of_cwd() {
    let outside = tempdir().unwrap();
    fs::write(outside.path().join("secrets.sh"), "open sesame please\n").unwrap();
    let inside = tempdir().unwrap();
    fs::write(inside.path().join("own.sh"), "open sesame please\n").unwrap();
    symlinked(inside.path(), "hop", outside.path());
    let context = create_test_context(inside.path());
    let root = context.working_directory.clone();

    let mut results = Vec::new();
    search_directory(
        &root,
        &root,
        "open sesame please",
        true,
        &mut results,
        SEARCH_MAX_RESULTS,
        &context,
    )
    .expect("a walk of cwd");

    let output = results.join("\n");
    assert!(output.contains("own.sh"), "{output}");
    assert!(
        !output.contains("secrets.sh"),
        "the walk left cwd: {output}"
    );
}

/// The directory case above is caught by `descendable`; a link to a *file*
/// takes the other branch, which read whatever `is_file` resolved to.
#[cfg(unix)]
#[test]
fn the_search_walk_does_not_read_a_symlink_to_a_file_out_of_cwd() {
    let outside = tempdir().unwrap();
    let secret = outside.path().join("secrets.rs");
    fs::write(&secret, "open sesame please\n").unwrap();
    let inside = tempdir().unwrap();
    fs::write(inside.path().join("own.rs"), "open sesame please\n").unwrap();
    symlinked(inside.path(), "hop.rs", &secret);
    let context = create_test_context(inside.path());
    let root = context.working_directory.clone();

    let mut results = Vec::new();
    search_directory(
        &root,
        &root,
        "open sesame please",
        true,
        &mut results,
        SEARCH_MAX_RESULTS,
        &context,
    )
    .expect("a walk of cwd");

    let output = results.join("\n");
    assert!(output.contains("own.rs"), "{output}");
    assert!(
        !output.contains("hop.rs"),
        "the walk read a file outside cwd through a symlink: {output}"
    );
}

#[tokio::test]
async fn test_write_file_path_traversal_blocked() {
    let directory = tempdir().unwrap();
    let tool = WriteFileTool;
    let context = create_test_context(directory.path());

    let result = tool
        .execute(
            serde_json::json!({"path": "../../../etc/passwd", "content": "malicious"}),
            &context,
        )
        .await;

    assert!(result.is_err());
    let error = result.unwrap_err();
    assert!(error.to_string().contains("traversal"));
}

#[tokio::test]
async fn test_write_file_absolute_path_blocked() {
    let directory = tempdir().unwrap();
    let tool = WriteFileTool;
    let context = create_test_context(directory.path());

    let result = tool
        .execute(
            serde_json::json!({"path": "/etc/passwd", "content": "malicious"}),
            &context,
        )
        .await;

    assert!(result.is_err());
    let error = result.unwrap_err();
    assert!(error.to_string().contains("traversal"));
}

#[tokio::test]
async fn test_write_file_backslash_traversal_blocked() {
    let directory = tempdir().unwrap();
    let tool = WriteFileTool;
    let context = create_test_context(directory.path());

    let result = tool
        .execute(
            serde_json::json!({"path": "..\\..\\file.txt", "content": "malicious"}),
            &context,
        )
        .await;

    assert!(result.is_err());
    let error = result.unwrap_err();
    assert!(error.to_string().contains("traversal"));
}

#[tokio::test]
async fn test_write_file_nested_traversal_blocked() {
    let directory = tempdir().unwrap();
    let tool = WriteFileTool;
    let context = create_test_context(directory.path());

    let result = tool
        .execute(
            serde_json::json!({"path": "subdir/../../../secret.txt", "content": "malicious"}),
            &context,
        )
        .await;

    assert!(result.is_err());
    let error = result.unwrap_err();
    assert!(error.to_string().contains("traversal"));
}

#[test]
fn test_list_files_tool_metadata() {
    let tool = ListFilesTool;
    assert_eq!(tool.name(), "list_files");
    assert!(!tool.description().is_empty());
}

#[tokio::test]
async fn test_list_files_success() {
    let directory = tempdir().unwrap();
    fs::write(directory.path().join("file1.txt"), "").unwrap();
    fs::write(directory.path().join("file2.rs"), "").unwrap();
    fs::create_dir(directory.path().join("subdir")).unwrap();

    let tool = ListFilesTool;
    let context = create_test_context(directory.path());

    let result = tool
        .execute(serde_json::json!({"path": "."}), &context)
        .await
        .unwrap();

    assert!(result.success);
    let output = result.output.unwrap();
    assert!(output.contains("file1.txt"));
    assert!(output.contains("file2.rs"));
    assert!(output.contains("subdir/"));
}

#[tokio::test]
async fn test_list_files_with_pattern() {
    let directory = tempdir().unwrap();
    fs::write(directory.path().join("file1.txt"), "").unwrap();
    fs::write(directory.path().join("file2.rs"), "").unwrap();
    fs::write(directory.path().join("file3.txt"), "").unwrap();

    let tool = ListFilesTool;
    let context = create_test_context(directory.path());

    let result = tool
        .execute(
            serde_json::json!({"path": ".", "pattern": "*.txt"}),
            &context,
        )
        .await
        .unwrap();

    assert!(result.success);
    let output = result.output.unwrap();
    assert!(output.contains("file1.txt"));
    assert!(output.contains("file3.txt"));
    assert!(!output.contains("file2.rs"));
}

#[tokio::test]
async fn list_files_recursive_caps_output() {
    let directory = tempdir().unwrap();
    for i in 0..250 {
        let nested = directory.path().join(format!("n{}/deep", i % 10));
        fs::create_dir_all(&nested).unwrap();
        fs::write(nested.join(format!("f{i}.txt")), "").unwrap();
    }

    let tool = ListFilesTool;
    let context = create_test_context(directory.path());
    let result = tool
        .execute(
            serde_json::json!({"path": ".", "recursive": true}),
            &context,
        )
        .await
        .unwrap();

    assert!(result.success);
    let output = result.output.unwrap();
    let paths: Vec<&str> = output
        .lines()
        .filter(|line| !line.starts_with('['))
        .collect();
    assert_eq!(paths.len(), LIST_FILES_CAP);
    assert!(output.contains("[truncated; omitted=50]"), "{output}");
}

#[test]
fn test_search_code_tool_metadata() {
    let tool = SearchCodeTool;
    assert_eq!(tool.name(), "search_code");
    assert!(!tool.description().is_empty());
}

#[tokio::test]
async fn test_search_code_success() {
    let directory = tempdir().unwrap();
    fs::write(
        directory.path().join("test.rs"),
        "fn main() {\n    println!(\"Hello\");\n}",
    )
    .unwrap();
    fs::write(
        directory.path().join("other.rs"),
        "fn other() {\n    // nothing\n}",
    )
    .unwrap();

    let tool = SearchCodeTool;
    let context = create_test_context(directory.path());

    let result = tool
        .execute(serde_json::json!({"pattern": "println"}), &context)
        .await
        .unwrap();

    assert!(result.success);
    let output = result.output.unwrap();
    assert!(output.contains("test.rs"));
    assert!(output.contains("println"));
}

#[tokio::test]
async fn test_search_code_case_insensitive() {
    let directory = tempdir().unwrap();
    fs::write(directory.path().join("test.rs"), "fn HELLO() {}").unwrap();

    let tool = SearchCodeTool;
    let context = create_test_context(directory.path());

    let result = tool
        .execute(
            serde_json::json!({"pattern": "hello", "case_sensitive": false}),
            &context,
        )
        .await
        .unwrap();

    assert!(result.success);
    let output = result.output.unwrap();
    assert!(output.contains("HELLO"));
}

#[tokio::test]
async fn search_code_respects_max_results() {
    let directory = tempdir().unwrap();
    fs::write(
        directory.path().join("many.rs"),
        "hit one\nhit two\nhit three\nhit four\n",
    )
    .unwrap();
    let tool = SearchCodeTool;
    let context = create_test_context(directory.path());
    let result = tool
        .execute(
            serde_json::json!({"pattern": "hit", "max_results": 2}),
            &context,
        )
        .await
        .unwrap();
    assert!(result.success);
    let output = result.output.unwrap();
    assert!(output.contains("truncated at 2 results"), "{output}");
    assert_eq!(output.matches("hit ").count(), 2);
}

#[tokio::test]
async fn search_code_ignores_huge_max_results() {
    let directory = tempdir().unwrap();
    let mut content = String::new();
    for i in 0..150 {
        content.push_str(&format!("hit {i}\n"));
    }
    fs::write(directory.path().join("many.rs"), content).unwrap();
    let tool = SearchCodeTool;
    let context = create_test_context(directory.path());
    let result = tool
        .execute(
            serde_json::json!({"pattern": "hit", "max_results": 1_000_000}),
            &context,
        )
        .await
        .unwrap();
    assert!(result.success);
    let output = result.output.unwrap();
    assert!(output.contains("truncated at 100 results"), "{output}");
    assert_eq!(output.matches("hit ").count(), SEARCH_MAX_RESULTS);
}

#[tokio::test]
async fn search_code_missing_path_is_no_matches() {
    let directory = tempdir().unwrap();
    let tool = SearchCodeTool;
    let context = create_test_context(directory.path());
    let result = tool
        .execute(
            serde_json::json!({"pattern": "anything", "path": "does-not-exist"}),
            &context,
        )
        .await
        .unwrap();
    assert!(result.success);
    assert!(result.output.unwrap().contains("No matches found"));
}

#[tokio::test]
async fn test_search_code_no_matches() {
    let directory = tempdir().unwrap();
    fs::write(directory.path().join("test.rs"), "fn main() {}").unwrap();

    let tool = SearchCodeTool;
    let context = create_test_context(directory.path());

    let result = tool
        .execute(
            serde_json::json!({"pattern": "nonexistent_pattern_xyz"}),
            &context,
        )
        .await
        .unwrap();

    assert!(result.success);
    assert!(result.output.unwrap().contains("No matches found"));
}

#[test]
fn test_tool_definitions() {
    let read = ReadFileTool;
    let definition = read.to_definition();
    assert_eq!(definition.tool_type, "function");
    assert_eq!(definition.function.name, "read_file");

    let write = WriteFileTool;
    let definition = write.to_definition();
    assert_eq!(definition.function.name, "write_file");

    let list = ListFilesTool;
    let definition = list.to_definition();
    assert_eq!(definition.function.name, "list_files");

    let search = SearchCodeTool;
    let definition = search.to_definition();
    assert_eq!(definition.function.name, "search_code");

    let patch = ApplyPatchTool;
    let definition = patch.to_definition();
    assert_eq!(definition.function.name, "apply_patch");
    assert_eq!(patch.tier(), Tier::Host);
    assert_eq!(ReadFileTool.tier(), Tier::Read);
}

#[tokio::test]
async fn apply_patch_replaces_unique_text() {
    let directory = tempdir().unwrap();
    fs::write(
        directory.path().join("main.rs"),
        "fn main() {\n    println!(\"a\");\n}\n",
    )
    .unwrap();
    let tool = ApplyPatchTool;
    let context = create_test_context(directory.path());

    let result = tool
        .execute(
            serde_json::json!({
                "path": "main.rs",
                "old_string": "println!(\"a\");",
                "new_string": "println!(\"b\");"
            }),
            &context,
        )
        .await
        .unwrap();

    assert!(result.success, "{:?}", result.error);
    assert_eq!(
        fs::read_to_string(directory.path().join("main.rs")).unwrap(),
        "fn main() {\n    println!(\"b\");\n}\n"
    );
}

#[tokio::test]
async fn apply_patch_rejects_ambiguous_matches() {
    let directory = tempdir().unwrap();
    fs::write(directory.path().join("dup.txt"), "foo\nfoo\n").unwrap();
    let tool = ApplyPatchTool;
    let context = create_test_context(directory.path());

    let result = tool
        .execute(
            serde_json::json!({
                "path": "dup.txt",
                "old_string": "foo",
                "new_string": "bar"
            }),
            &context,
        )
        .await;

    assert!(result.unwrap_err().to_string().contains("matched 2 times"));
    assert_eq!(
        fs::read_to_string(directory.path().join("dup.txt")).unwrap(),
        "foo\nfoo\n"
    );
}

#[tokio::test]
async fn apply_patch_replace_all_and_hunks() {
    let directory = tempdir().unwrap();
    fs::write(directory.path().join("dup.txt"), "foo\nfoo\nbaz\n").unwrap();
    let tool = ApplyPatchTool;
    let context = create_test_context(directory.path());

    let result = tool
        .execute(
            serde_json::json!({
                "path": "dup.txt",
                "replace_all": true,
                "hunks": [
                    {"old_string": "foo", "new_string": "bar"},
                    {"old_string": "baz", "new_string": "qux"}
                ]
            }),
            &context,
        )
        .await
        .unwrap();

    assert!(result.success, "{:?}", result.error);
    assert_eq!(
        fs::read_to_string(directory.path().join("dup.txt")).unwrap(),
        "bar\nbar\nqux\n"
    );
}

#[test]
fn write_file_params_read_the_reason() {
    let parameters: WriteFileParameters = serde_json::from_value(serde_json::json!({
        "path": "src/main.rs",
        "content": "fn main() {}\n",
        "reason": "Create the binary entry point the crate is missing."
    }))
    .unwrap();

    assert_eq!(
        parameters.reason.as_deref(),
        Some("Create the binary entry point the crate is missing.")
    );
}

#[tokio::test]
async fn write_file_accepts_a_call_carrying_a_reason() {
    let directory = tempdir().unwrap();
    let context = create_test_context(directory.path());

    let result = WriteFileTool
        .execute(
            serde_json::json!({
                "path": "notes.txt",
                "content": "hello\n",
                "reason": "Record the note the user asked for."
            }),
            &context,
        )
        .await
        .unwrap();

    assert!(result.success, "{:?}", result.error);
    assert_eq!(
        fs::read_to_string(directory.path().join("notes.txt")).unwrap(),
        "hello\n"
    );
}

#[tokio::test]
async fn write_file_without_a_reason_still_writes() {
    let directory = tempdir().unwrap();
    let context = create_test_context(directory.path());

    let result = WriteFileTool
        .execute(
            serde_json::json!({"path": "notes.txt", "content": "hello\n"}),
            &context,
        )
        .await
        .unwrap();

    assert!(result.success, "{:?}", result.error);
    assert_eq!(
        fs::read_to_string(directory.path().join("notes.txt")).unwrap(),
        "hello\n"
    );
}

#[test]
fn apply_patch_params_read_the_reason() {
    let parameters: ApplyPatchParameters = serde_json::from_value(serde_json::json!({
        "path": "src/main.rs",
        "old_string": "foo",
        "new_string": "bar",
        "reason": "Rename the helper the caller now expects."
    }))
    .unwrap();

    assert_eq!(
        parameters.reason.as_deref(),
        Some("Rename the helper the caller now expects.")
    );
}

#[tokio::test]
async fn apply_patch_accepts_a_call_carrying_a_reason() {
    let directory = tempdir().unwrap();
    fs::write(directory.path().join("a.txt"), "foo\n").unwrap();
    let context = create_test_context(directory.path());

    let result = ApplyPatchTool
        .execute(
            serde_json::json!({
                "path": "a.txt",
                "old_string": "foo",
                "new_string": "bar",
                "reason": "Rename the helper the caller now expects."
            }),
            &context,
        )
        .await
        .unwrap();

    assert!(result.success, "{:?}", result.error);
    assert_eq!(
        fs::read_to_string(directory.path().join("a.txt")).unwrap(),
        "bar\n"
    );
}

#[tokio::test]
async fn apply_patch_without_a_reason_still_patches() {
    let directory = tempdir().unwrap();
    fs::write(directory.path().join("a.txt"), "foo\n").unwrap();
    let context = create_test_context(directory.path());

    let result = ApplyPatchTool
        .execute(
            serde_json::json!({"path": "a.txt", "old_string": "foo", "new_string": "bar"}),
            &context,
        )
        .await
        .unwrap();

    assert!(result.success, "{:?}", result.error);
    assert_eq!(
        fs::read_to_string(directory.path().join("a.txt")).unwrap(),
        "bar\n"
    );
}

/// A reason is the model's own prose and can carry whatever it just read
/// out of a file or a page, so the run log records that one arrived and
/// never what it said.
#[tokio::test]
async fn the_writing_tools_log_that_a_reason_arrived_without_repeating_it() {
    const LIFTED: &str = "AWS_SECRET_ACCESS_KEY read out of the .env I just opened";
    let directory = tempdir().unwrap();
    fs::write(directory.path().join("a.txt"), "foo\n").unwrap();
    let context = create_test_context(directory.path());

    let (_, write_log) = captured_logs(WriteFileTool.execute(
        serde_json::json!({"path": "b.txt", "content": "hi", "reason": LIFTED}),
        &context,
    ))
    .await;
    let (_, patch_log) = captured_logs(ApplyPatchTool.execute(
        serde_json::json!({
            "path": "a.txt",
            "old_string": "foo",
            "new_string": "bar",
            "reason": LIFTED
        }),
        &context,
    ))
    .await;

    for (tool, logged) in [("write_file", write_log), ("apply_patch", patch_log)] {
        assert!(logged.contains("Running tool"), "{logged}");
        assert!(logged.contains(tool), "{logged}");
        assert!(logged.contains("reason_given=true"), "{logged}");
        assert!(
            !logged.contains(LIFTED),
            "{tool} wrote the model's reason to the log: {logged}"
        );
    }
}

#[tokio::test]
async fn a_writing_tool_without_a_reason_logs_none_given() {
    let directory = tempdir().unwrap();
    let context = create_test_context(directory.path());

    let (_, logged) = captured_logs(WriteFileTool.execute(
        serde_json::json!({"path": "b.txt", "content": "hi"}),
        &context,
    ))
    .await;

    assert!(logged.contains("reason_given=false"), "{logged}");
}

#[tokio::test]
async fn apply_patch_rejects_missing_text() {
    let directory = tempdir().unwrap();
    fs::write(directory.path().join("a.txt"), "hello\n").unwrap();
    let tool = ApplyPatchTool;
    let context = create_test_context(directory.path());

    let result = tool
        .execute(
            serde_json::json!({
                "path": "a.txt",
                "old_string": "missing",
                "new_string": "x"
            }),
            &context,
        )
        .await;

    assert!(result.unwrap_err().to_string().contains("did not match"));
}

/// The window this closes: a tool checks where a path leads, and the entry
/// it checked is replaced with a symlink out of `cwd` before the open
/// resolves the same name a second time.
///
/// Both states arrive by renaming over the entry, which is atomic, so the
/// name never stops existing and never resolves to something half-made.
/// Every attempt gets past the check; only which file the open lands on is
/// in question.
#[cfg(unix)]
fn swapping(entry: PathBuf, escape: PathBuf, stop: Arc<AtomicBool>) -> JoinHandle<()> {
    let spare = entry.with_extension("spare");
    std::thread::spawn(move || {
        while !stop.load(Ordering::Relaxed) {
            let _ = fs::remove_file(&spare);
            if fs::write(&spare, INSIDE).is_ok() {
                let _ = fs::rename(&spare, &entry);
            }
            held();

            let _ = fs::remove_file(&spare);
            if std::os::unix::fs::symlink(&escape, &spare).is_ok() {
                let _ = fs::rename(&spare, &entry);
            }
            held();
        }
    })
}

/// Spun rather than slept: the gap being aimed at is a couple of syscalls
/// wide, far below the granularity a sleep can hold to.
#[cfg(unix)]
fn held() {
    let until = Instant::now() + HELD;
    while Instant::now() < until {
        std::hint::spin_loop();
    }
}

/// The relative name a link in `cwd/keep` uses to reach a file beside
/// `cwd`, so the escape is by `..` rather than by an absolute target.
#[cfg(unix)]
fn beside(outside: &Path) -> PathBuf {
    Path::new("../..")
        .join(outside.file_name().expect("a temporary directory name"))
        .join(SECRET_FILE)
}

/// `cwd/keep/leaf.rs` holding `INSIDE`, and the swapper aimed at it.
#[cfg(unix)]
fn swapped(context: &ToolContext, outside: &Path, stop: &Arc<AtomicBool>) -> JoinHandle<()> {
    let keep = context.working_directory.join(KEEP);
    fs::create_dir(&keep).expect("a directory inside cwd");
    let entry = keep.join(LEAF);
    fs::write(&entry, INSIDE).expect("a file inside cwd");
    swapping(entry, beside(outside), Arc::clone(stop))
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn read_file_never_returns_a_file_swapped_in_after_the_check() {
    let outside = tempdir().unwrap();
    fs::write(outside.path().join(SECRET_FILE), SECRET).unwrap();
    let inside = tempdir().unwrap();
    let context = create_test_context(inside.path());

    let stop = Arc::new(AtomicBool::new(false));
    let swapper = swapped(&context, outside.path(), &stop);

    let mut disclosed = None;
    for _ in 0..ATTEMPTS {
        let result = ReadFileTool
            .execute(
                serde_json::json!({"path": format!("{KEEP}/{LEAF}")}),
                &context,
            )
            .await;
        if let Ok(result) = result
            && let Some(output) = result.output
            && output.contains(SECRET)
        {
            disclosed = Some(output);
            break;
        }
    }

    stop.store(true, Ordering::Relaxed);
    swapper.join().unwrap();

    assert!(
        disclosed.is_none(),
        "read_file returned a file swapped in after its check: {disclosed:?}"
    );
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn write_file_never_follows_an_entry_swapped_in_after_the_check() {
    let outside = tempdir().unwrap();
    let secret = outside.path().join(SECRET_FILE);
    fs::write(&secret, SECRET).unwrap();
    let inside = tempdir().unwrap();
    let context = create_test_context(inside.path());

    let stop = Arc::new(AtomicBool::new(false));
    let swapper = swapped(&context, outside.path(), &stop);

    for _ in 0..ATTEMPTS {
        let _ = WriteFileTool
            .execute(
                serde_json::json!({
                    "path": format!("{KEEP}/{LEAF}"),
                    "content": INSIDE,
                    "reason": "the swap"
                }),
                &context,
            )
            .await;
    }

    stop.store(true, Ordering::Relaxed);
    swapper.join().unwrap();

    assert_eq!(
        fs::read_to_string(&secret).unwrap(),
        SECRET,
        "write_file wrote through an entry swapped in after its check"
    );
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn search_code_never_reads_an_entry_swapped_out_of_cwd() {
    let outside = tempdir().unwrap();
    fs::write(outside.path().join(SECRET_FILE), SECRET).unwrap();
    let inside = tempdir().unwrap();
    let context = create_test_context(inside.path());

    let stop = Arc::new(AtomicBool::new(false));
    let swapper = swapped(&context, outside.path(), &stop);

    let mut disclosed = Vec::new();
    for _ in 0..ATTEMPTS {
        let mut results = Vec::new();
        let _ = search_directory(
            &context.working_directory,
            &context.working_directory,
            SECRET,
            true,
            &mut results,
            SEARCH_MAX_RESULTS,
            &context,
        );
        if !results.is_empty() {
            disclosed = results;
            break;
        }
    }

    stop.store(true, Ordering::Relaxed);
    swapper.join().unwrap();

    assert!(
        disclosed.is_empty(),
        "the search walker read an entry swapped out of cwd: {disclosed:?}"
    );
}
