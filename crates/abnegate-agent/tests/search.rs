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

fn checkout() -> TempDir {
    let directory = TempDir::new().expect("a temporary checkout");
    std::fs::write(directory.path().join("main.rs"), "fn needle() {}\n").expect("a source file");
    directory
}

fn context(directory: &Path) -> ToolContext {
    ToolContext::default().within(directory.canonicalize().expect("a canonical checkout"))
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

#[tokio::test]
async fn a_search_given_no_time_stops_early_and_says_so() {
    let directory = checkout();
    let context = context(directory.path())
        .with_environment(EnvironmentPolicy::empty())
        .with_search_timeout(Duration::ZERO);

    let output = search(&context).await;

    assert!(output.contains(STOPPED_EARLY), "{output}");
    assert!(!output.contains("main.rs"), "{output}");
}

#[tokio::test]
async fn a_search_that_may_use_ripgrep_given_no_time_stops_early_too() {
    let directory = checkout();
    let context = context(directory.path()).with_search_timeout(Duration::ZERO);

    let output = search(&context).await;

    assert!(output.contains(STOPPED_EARLY), "{output}");
}

#[tokio::test]
async fn a_search_within_its_time_finds_everything() {
    let directory = checkout();
    let context = context(directory.path()).with_search_timeout(Duration::from_secs(30));

    let output = search(&context).await;

    assert!(output.contains("main.rs:1: fn needle() {}"), "{output}");
    assert!(!output.contains(STOPPED_EARLY), "{output}");
}
