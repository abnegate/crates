//! How long `search_code` is given, set from outside the crate.

use std::path::Path;
use std::time::Duration;

use abnegate_agent::Tool;
use abnegate_agent::ToolContext;
use abnegate_agent::tool::EnvironmentPolicy;
use abnegate_agent::tool::SearchCodeTool;
use serde_json::json;
use tempfile::TempDir;

const STOPPED_EARLY: &str = "search stopped early";

const SOURCE: &str = "main.rs";

/// A file ripgrep reads and the walk, which reads only code files, does not.
const NOTES: &str = "notes.data";

const PATIENCE: Duration = Duration::from_secs(30);

fn checkout() -> TempDir {
    let directory = TempDir::new().expect("a temporary checkout");
    std::fs::write(directory.path().join(SOURCE), "fn needle() {}\n").expect("a source file");
    directory
}

fn context(directory: &Path) -> ToolContext {
    ToolContext::default().within(directory.canonicalize().expect("a canonical checkout"))
}

/// A context whose searches of `directory` have to walk it, and the empty
/// directory that is all its children find on their `PATH`. An empty
/// environment alone would not do: the lookup then falls back to the host's
/// own default path, which on Linux holds `/usr/bin/rg`.
fn walking(directory: &Path) -> (TempDir, ToolContext) {
    let programs = TempDir::new().expect("a directory holding no ripgrep");
    let environment =
        EnvironmentPolicy::empty().with("PATH", programs.path().display().to_string());
    (programs, context(directory).with_environment(environment))
}

async fn search(context: &ToolContext) -> String {
    let result = SearchCodeTool
        .execute(json!({ "pattern": "needle" }), context)
        .await
        .expect("the search runs");
    assert!(result.success, "{result:?}");
    result.output.expect("the search reports")
}

#[test]
fn the_default_search_timeout_is_twenty_seconds() {
    assert_eq!(
        ToolContext::default().search_timeout,
        Duration::from_secs(20)
    );
}

/// Out of time, ripgrep and the walk answer alike, so only a search given
/// time shows which of them ran: ripgrep would find the match in the file
/// that is not code, and the walk never reads it.
#[tokio::test]
async fn a_search_without_ripgrep_walks_the_code_files_only() {
    let directory = checkout();
    std::fs::write(directory.path().join(NOTES), "needle\n").expect("a file that is not code");
    let (_programs, context) = walking(directory.path());

    let output = search(&context.with_search_timeout(PATIENCE)).await;

    assert!(output.contains(SOURCE), "{output}");
    assert!(
        !output.contains(NOTES),
        "ripgrep ran where none should be found: {output}"
    );
    assert!(!output.contains(STOPPED_EARLY), "{output}");
}

#[tokio::test]
async fn a_search_without_ripgrep_given_no_time_stops_early_and_says_so() {
    let directory = checkout();
    let (_programs, context) = walking(directory.path());

    let output = search(&context.with_search_timeout(Duration::ZERO)).await;

    assert!(output.contains(STOPPED_EARLY), "{output}");
    assert!(!output.contains(SOURCE), "{output}");
}

#[tokio::test]
async fn a_search_that_may_use_ripgrep_given_no_time_stops_early_too() {
    let directory = checkout();
    let context = context(directory.path()).with_search_timeout(Duration::ZERO);

    let output = search(&context).await;

    assert!(output.contains(STOPPED_EARLY), "{output}");
    assert!(!output.contains(SOURCE), "{output}");
}

#[tokio::test]
async fn a_search_within_its_time_finds_everything() {
    let directory = checkout();
    let context = context(directory.path()).with_search_timeout(PATIENCE);

    let output = search(&context).await;

    assert!(
        output.contains(&format!("{SOURCE}:1: fn needle() {{}}")),
        "{output}"
    );
    assert!(!output.contains(STOPPED_EARLY), "{output}");
}
