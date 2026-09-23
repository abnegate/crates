//! Waiting for a background job to end.

mod parameters;

use async_trait::async_trait;
use serde_json::{Value, json};
use std::time::Duration;

use super::job::{JobExited, Jobs, MAX_JOB_LIFETIME, TAIL_JOB, WAIT_FOR};
use super::{TIMEOUT_SLACK, Tier, Tool, ToolContext, ToolError, ToolResult};
use parameters::WaitForParameters;

const ID_PARAMETER: &str = "id";
const TIMEOUT_PARAMETER: &str = "timeout_secs";

/// How long a wait lasts when the call names no limit.
const DEFAULT_WAIT: Duration = Duration::from_secs(120);

/// Block until a background job ends, or a limit passes.
///
/// The receipt for a background call points the model here: waiting belongs
/// in one call that can see the job, not in a `sleep` the shell tool refuses
/// past a minute.
pub struct WaitForTool;

fn ended(exited: &JobExited) -> ToolResult {
    let footer = format!("Read its output with {TAIL_JOB}.");
    match exited.exit_code {
        Some(code) => ToolResult::success(format!(
            "Job {} exited with code {code}. {footer}",
            exited.id
        )),
        None => ToolResult::error(format!(
            "Job {} was stopped before it finished: it was killed, outlived its limit or \
             filled its log. {footer}",
            exited.id
        )),
    }
}

#[async_trait]
impl Tool for WaitForTool {
    fn name(&self) -> &str {
        WAIT_FOR
    }

    fn description(&self) -> &str {
        "Wait for a background job to end and report how it ended. Returns early, saying the job \
         is still running, once the limit passes."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                ID_PARAMETER: {
                    "type": "string",
                    "description": "Job id from a background run_shell or run_command."
                },
                TIMEOUT_PARAMETER: {
                    "type": "integer",
                    "minimum": 1,
                    "description": format!(
                        "Longest to wait, in seconds. Default {}, at most {}.",
                        DEFAULT_WAIT.as_secs(),
                        MAX_JOB_LIFETIME.as_secs()
                    )
                }
            },
            "required": [ID_PARAMETER],
            "additionalProperties": false
        })
    }

    fn tier(&self) -> Tier {
        Tier::Read
    }

    fn timeout(&self, _context: &ToolContext) -> Duration {
        MAX_JOB_LIFETIME + TIMEOUT_SLACK
    }

    async fn execute(
        &self,
        parameters: Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let parameters: WaitForParameters = serde_json::from_value(parameters)
            .map_err(|error| ToolError::InvalidParameters(error.to_string()))?;
        let limit = parameters
            .timeout_seconds
            .map_or(DEFAULT_WAIT, Duration::from_secs)
            .clamp(Duration::from_secs(1), MAX_JOB_LIFETIME);

        let settled =
            Jobs::settled(context.session, &parameters.id).map_err(ToolError::Execution)?;
        Ok(match tokio::time::timeout(limit, settled).await {
            Ok(exited) => ended(&exited),
            Err(_) => ToolResult::success(format!(
                "Job {} is still running after {} seconds. Wait again with {WAIT_FOR}, or read \
                 it so far with {TAIL_JOB}.",
                parameters.id,
                limit.as_secs()
            )),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::job::{UNAVAILABLE, parse_started};
    use crate::tools::{EnvironmentPolicy, RunShellTool, Session};
    use uuid::Uuid;

    fn context(session: Session, directory: &std::path::Path) -> ToolContext {
        let mut context = ToolContext::default().within(directory);
        context.session = session;
        context.unrestricted = true;
        context.environment =
            EnvironmentPolicy::empty().with("PATH", std::env::var("PATH").unwrap_or_default());
        context
    }

    async fn started(line: &str, context: &ToolContext) -> String {
        let receipt = RunShellTool
            .execute(
                json!({"command": line, "background": true, "reason": "Start it."}),
                context,
            )
            .await
            .expect("the job starts");
        parse_started(&receipt.output.expect("a receipt")).expect("a job id")
    }

    #[tokio::test]
    async fn waiting_returns_how_the_job_ended() {
        let directory = tempfile::tempdir().unwrap();
        let session = Session::Chat(Uuid::new_v4());
        let context = context(session, directory.path());
        let id = started("exit 3", &context).await;

        let result = WaitForTool
            .execute(json!({ ID_PARAMETER: id.clone() }), &context)
            .await
            .unwrap();

        assert!(result.success, "{result:?}");
        assert!(
            result
                .output
                .unwrap()
                .contains(&format!("Job {id} exited with code 3")),
        );
        Jobs::kill_session(session).await;
    }

    #[tokio::test]
    async fn a_wait_past_its_limit_says_the_job_is_still_running() {
        let directory = tempfile::tempdir().unwrap();
        let session = Session::Chat(Uuid::new_v4());
        let context = context(session, directory.path());
        let id = started("sleep 30", &context).await;

        let result = WaitForTool
            .execute(json!({ ID_PARAMETER: id, TIMEOUT_PARAMETER: 1 }), &context)
            .await
            .unwrap();

        assert!(
            result
                .output
                .unwrap()
                .contains("still running after 1 seconds")
        );
        Jobs::kill_session(session).await;
    }

    #[tokio::test]
    async fn a_detached_context_has_no_job_to_wait_for() {
        let directory = tempfile::tempdir().unwrap();
        let error = WaitForTool
            .execute(
                json!({ ID_PARAMETER: "job_000000000000" }),
                &context(Session::Detached, directory.path()),
            )
            .await
            .expect_err("nothing to wait for");
        assert!(error.to_string().contains(UNAVAILABLE), "{error}");
    }

    #[test]
    fn the_outer_timeout_covers_the_longest_wait() {
        assert!(WaitForTool.timeout(&ToolContext::default()) > MAX_JOB_LIFETIME);
        assert_eq!(WaitForTool.tier(), Tier::Read);
    }
}
