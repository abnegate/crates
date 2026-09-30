use std::sync::Arc;

use abnegate_agent::Tool;
use abnegate_agent::ToolContext;
use abnegate_agent::ToolRegistry;
use abnegate_agent::tool::ApplyPatchTool;
use abnegate_agent::tool::ListFilesTool;
use abnegate_agent::tool::ReadFileTool;
use abnegate_agent::tool::RunCommandTool;
use abnegate_agent::tool::RunShellTool;
use abnegate_agent::tool::SearchCodeTool;
use abnegate_agent::tool::WaitFor;
use abnegate_agent::tool::WaitForTool;
use abnegate_agent::tool::WriteFileTool;
use abnegate_agent::tool::job::JobStarted;
use abnegate_agent::tool::job::WAIT_FOR;
use abnegate_agent::tool::job::parse_receipt;
use abnegate_agent::tool::job::receipt;
use abnegate_agent::tool::job::started_text;
use abnegate_agent::tool::tail::TailJobTool;
use serde_json::json;

#[test]
fn a_job_built_outside_the_crate_round_trips_through_its_receipt() {
    let job = JobStarted::new(
        "job_9f3c1a7b2e04",
        48213,
        "/tmp/work/.abnegate/jobs/job_9f3c1a7b2e04.log",
    );

    assert_eq!(job.id, "job_9f3c1a7b2e04");
    assert_eq!(job.pid, 48213);
    assert_eq!(
        job.log_path,
        "/tmp/work/.abnegate/jobs/job_9f3c1a7b2e04.log"
    );
    assert_eq!(parse_receipt(&started_text(&job)), Some(job.clone()));
    for wait_for in WaitFor::ALL {
        assert_eq!(parse_receipt(&receipt(&job, *wait_for)), Some(job.clone()));
    }
}

/// The host tools without `wait_for`, as a caller whose turns cannot park
/// builds them.
fn without_wait_for() -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(ReadFileTool));
    registry.register(Arc::new(WriteFileTool));
    registry.register(Arc::new(ApplyPatchTool));
    registry.register(Arc::new(ListFilesTool));
    registry.register(Arc::new(SearchCodeTool));
    registry.register(Arc::new(RunCommandTool));
    registry.register(Arc::new(RunShellTool));
    registry.register(Arc::new(TailJobTool));
    registry
}

/// Everything a registry's tools tell the model: every definition, and the
/// refusals a shell call that sleeps too long reads, backgrounded or not.
async fn served(registry: &ToolRegistry) -> Vec<String> {
    let mut texts: Vec<String> = registry
        .definitions()
        .iter()
        .map(|definition| serde_json::to_string(definition).expect("a definition"))
        .collect();
    for background in [false, true] {
        let refused = registry
            .execute(
                "run_shell",
                json!({
                    "command": "sleep 600",
                    "background": background,
                    "reason": "Wait for the deploy."
                }),
                &ToolContext::default(),
            )
            .await
            .expect_err("a sleep past the cap is refused");
        texts.push(refused.to_string());
    }
    texts
}

fn hedged(text: &str) -> usize {
    text.matches(WaitFor::Withheld.condition()).count()
}

/// A caller that leaves `wait_for` out of its registry serves turns that
/// cannot call it, so every instruction to call it says it needs the tool,
/// and a registry that holds it says so nowhere, whichever was registered
/// first.
#[tokio::test]
async fn a_registry_without_wait_for_never_tells_the_model_to_call_it() {
    let mut registry = without_wait_for();

    let waits: Vec<String> = served(&registry)
        .await
        .into_iter()
        .filter(|text| text.contains(WAIT_FOR))
        .collect();
    assert!(waits.len() >= 4, "{waits:?}");
    for text in &waits {
        assert_eq!(text.matches(WAIT_FOR).count(), hedged(text), "{text}");
    }
    let shell = registry.get("run_shell").expect("a shell is registered");
    assert_eq!(
        serde_json::to_string(&shell.to_definition()).expect("a definition"),
        serde_json::to_string(
            &RunShellTool
                .unwaited()
                .expect("an unwaited shell")
                .to_definition()
        )
        .expect("a definition"),
        "the tool a caller looks up is the one the model was offered"
    );

    registry.register(Arc::new(WaitForTool));

    for text in served(&registry).await {
        assert_eq!(hedged(&text), 0, "{text}");
    }
}
