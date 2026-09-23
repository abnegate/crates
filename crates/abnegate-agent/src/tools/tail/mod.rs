//! Reading the log of a background job.
//!
//! The cursor is a byte offset rather than a line number, because the log is a
//! file: an offset is a seek, a line number is a scan of everything before it,
//! and a cursor the reader cannot place is one the tool has to reject.

mod params;

use async_trait::async_trait;
use serde_json::{Value, json};

use super::command::{MAX_OUTPUT_PARAM, clamp_output_chars, max_output_property};
use super::job::{JobState, JobTail, Jobs, TAIL_JOB};
use super::{Tier, Tool, ToolContext, ToolError, ToolResult};
use params::TailJobParams;

const ID_PARAM: &str = "id";
const SINCE_PARAM: &str = "since";

/// Read what a background job has written so far.
pub struct TailJobTool;

/// The slice, then where the job has got to and where the next read starts.
///
/// A flooded job is reported as a failure: it was killed for filling the disk,
/// and a success would read as a command that ran to the end.
fn report(tail: JobTail) -> ToolResult {
    let footer = format!("[job {}; next={}]", tail.state, tail.next);
    let mut slice = tail.output;
    if !slice.is_empty() && !slice.ends_with('\n') {
        slice.push('\n');
    }
    slice.push_str(&footer);

    match tail.state {
        JobState::Flooded => ToolResult::error(slice),
        _ => ToolResult::success(slice),
    }
}

#[async_trait]
impl Tool for TailJobTool {
    fn name(&self) -> &str {
        TAIL_JOB
    }

    fn description(&self) -> &str {
        "Read a background job's log. Returns what the job has written, then a line saying where \
         it has got to and where the next read starts. Pass that offset back as `since` to see \
         only what is new."
    }

    fn tier(&self) -> Tier {
        Tier::Read
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                ID_PARAM: {
                    "type": "string",
                    "description": "Job id from a background run_shell or run_command."
                },
                SINCE_PARAM: {
                    "type": "integer",
                    // Parsed into a u64, so a negative fails the call rather
                    // than being read as a seek from the end.
                    "minimum": 0,
                    "description": format!(
                        "Byte offset returned as `next` by a previous {TAIL_JOB}. Omit to read \
                         from the start."
                    )
                },
                MAX_OUTPUT_PARAM: max_output_property()
            },
            "required": [ID_PARAM],
            "additionalProperties": false
        })
    }

    async fn execute(&self, params: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let params: TailJobParams = serde_json::from_value(params)
            .map_err(|error| ToolError::InvalidParams(error.to_string()))?;

        let tail = Jobs::read(
            context.session,
            &params.id,
            params.since.unwrap_or_default(),
            clamp_output_chars(params.max_output_chars),
        )
        .await
        .map_err(ToolError::Execution)?;

        Ok(report(tail))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::job::{UNAVAILABLE, parse_started};
    use crate::tools::{RunShellTool, Session};
    use std::path::Path;
    use std::time::Duration;
    use tempfile::TempDir;
    use uuid::Uuid;

    const POLL: Duration = Duration::from_millis(20);
    const POLL_LIMIT: usize = 500;

    fn directory() -> TempDir {
        TempDir::new().expect("a temporary working directory")
    }

    fn context(cwd: &Path, session: Session) -> ToolContext {
        ToolContext {
            cwd: cwd.to_path_buf(),
            session,
            unrestricted: true,
            ..ToolContext::default()
        }
    }

    fn chat() -> Session {
        Session::Chat(Uuid::new_v4())
    }

    /// Start a job the way the model does, through the shell tool.
    async fn spawned(line: &str, context: &ToolContext) -> String {
        let result = RunShellTool
            .execute(
                json!({
                    "command": line,
                    "background": true,
                    "reason": "Start the long one."
                }),
                context,
            )
            .await
            .expect("the job starts");
        assert!(result.success, "{result:?}");
        let output = result.output.expect("a receipt");
        parse_started(&output).expect("the receipt names the job")
    }

    async fn tail(id: &str, since: u64, context: &ToolContext) -> ToolResult {
        TailJobTool
            .execute(json!({ ID_PARAM: id, SINCE_PARAM: since }), context)
            .await
            .expect("its own session reads it")
    }

    /// Read the whole log until the footer says the job has stopped.
    async fn settled(id: &str, context: &ToolContext) -> String {
        for _ in 0..POLL_LIMIT {
            let slice = tail(id, 0, context).await.output.unwrap_or_default();
            if !slice.contains("[job running;") {
                return slice;
            }
            tokio::time::sleep(POLL).await;
        }
        panic!("{id} never settled");
    }

    #[tokio::test]
    async fn a_backgrounded_command_writes_a_log_this_tool_reads_back() {
        let directory = directory();
        let session = chat();
        let context = context(directory.path(), session);

        let job = spawned("printf 'first\\nsecond\\n'", &context).await;
        let report = settled(&job, &context).await;

        assert!(report.contains("first"), "{report}");
        assert!(report.contains("second"), "{report}");
        assert!(report.contains("[job exited 0; next="), "{report}");

        Jobs::kill_session(session).await;
    }

    /// The cursor addresses bytes, so reading from `next` returns what was
    /// written after the last read and nothing that was already seen.
    #[tokio::test]
    async fn since_is_a_byte_offset_that_reads_only_what_is_new() {
        let directory = directory();
        let session = chat();
        let context = context(directory.path(), session);

        let job = spawned("printf 'one\\ntwo\\n'", &context).await;
        settled(&job, &context).await;

        let whole = tail(&job, 0, &context).await.output.unwrap();
        assert!(whole.starts_with("one\ntwo\n"), "{whole}");
        assert!(whole.contains("; next=8]"), "{whole}");

        let rest = tail(&job, 8, &context).await.output.unwrap();
        assert_eq!(rest, "[job exited 0; next=8]", "nothing new was written");

        Jobs::kill_session(session).await;
    }

    #[tokio::test]
    async fn a_job_started_by_another_session_is_not_readable() {
        let directory = directory();
        let owner = chat();
        let owning = context(directory.path(), owner);
        let job = spawned("printf 'private\\n'", &owning).await;
        settled(&job, &owning).await;

        let stranger = context(directory.path(), chat());
        let error = TailJobTool
            .execute(json!({ ID_PARAM: job.clone() }), &stranger)
            .await
            .expect_err("another session cannot read it");

        assert!(
            error
                .to_string()
                .contains(&format!("No job {job} in this session.")),
            "{error}"
        );

        Jobs::kill_session(owner).await;
    }

    #[tokio::test]
    async fn a_detached_context_cannot_read_a_job() {
        let directory = directory();
        let owner = chat();
        let owning = context(directory.path(), owner);
        let job = spawned("printf 'private\\n'", &owning).await;

        let detached = context(directory.path(), Session::Detached);
        let error = TailJobTool
            .execute(json!({ ID_PARAM: job.clone() }), &detached)
            .await
            .expect_err("a detached context has no session to key a job to");

        assert!(error.to_string().contains(UNAVAILABLE), "{error}");

        Jobs::kill_session(owner).await;
    }

    /// A job killed for filling the disk produced no result, so the call it
    /// was read through must not read as one.
    #[test]
    fn a_flooded_job_is_reported_flooded_and_is_not_a_success() {
        let result = report(JobTail {
            output: "building\n".to_string(),
            state: JobState::Flooded,
            next: 9,
        });

        assert!(!result.success, "{result:?}");
        let error = result.error.expect("a failure carries its text");
        assert!(error.contains("building"), "{error}");
        assert!(error.ends_with("[job flooded; next=9]"), "{error}");
    }

    #[test]
    fn every_other_state_reaches_the_footer_the_job_spells_it_with() {
        for (state, spelling) in [
            (JobState::Running, "running"),
            (JobState::Exited(0), "exited 0"),
            (JobState::Exited(101), "exited 101"),
            (JobState::Killed, "killed"),
        ] {
            let result = report(JobTail {
                output: String::new(),
                state,
                next: 12,
            });
            assert!(result.success, "{state:?}");
            assert_eq!(
                result.output.expect("a slice"),
                format!("[job {spelling}; next=12]"),
                "{state:?}"
            );
        }
    }

    /// An empty slice still says where the job is, so a caller that reads
    /// nothing learns whether there is more coming.
    #[test]
    fn a_slice_and_its_footer_are_kept_on_separate_lines() {
        let unterminated = report(JobTail {
            output: "no newline".to_string(),
            state: JobState::Running,
            next: 10,
        });

        assert_eq!(
            unterminated.output.expect("a slice"),
            "no newline\n[job running; next=10]"
        );
    }

    #[test]
    fn the_schema_asks_only_for_a_job_id_and_takes_no_reason() {
        let schema = TailJobTool.parameters_schema();
        let properties = schema["properties"].as_object().expect("properties");

        assert_eq!(schema["required"].as_array().unwrap(), &[json!(ID_PARAM)]);
        assert_eq!(schema["additionalProperties"], json!(false));
        assert_eq!(
            properties[ID_PARAM]["description"],
            json!("Job id from a background run_shell or run_command.")
        );
        assert_eq!(properties[SINCE_PARAM]["minimum"], json!(0));
        assert_eq!(properties[SINCE_PARAM]["type"], json!("integer"));
        assert_eq!(
            properties[SINCE_PARAM]["description"],
            json!(
                "Byte offset returned as `next` by a previous tail_job. Omit to read from the \
                 start."
            )
        );
        assert!(properties.contains_key(MAX_OUTPUT_PARAM));
        assert!(
            !properties.contains_key(crate::tools::REASON_PARAM),
            "a read states no reason"
        );
    }

    /// Reading a log changes nothing and answers nothing, so it neither waits
    /// for a confirmation nor stops the turn it was called in.
    #[test]
    fn reading_a_log_is_a_read_that_does_not_end_a_turn() {
        assert_eq!(TailJobTool.name(), TAIL_JOB);
        assert_eq!(TailJobTool.tier(), Tier::Read);
        assert!(!TailJobTool.ends_turn());
        assert!(
            TailJobTool
                .preview(&json!({ ID_PARAM: "job_9f3c1a7b2e04" }))
                .is_none()
        );
    }

    #[tokio::test]
    async fn an_unreadable_cap_is_clamped_rather_than_refused() {
        let directory = directory();
        let session = chat();
        let context = context(directory.path(), session);

        let job = spawned("printf 'kept\\n'", &context).await;
        settled(&job, &context).await;

        let result = TailJobTool
            .execute(
                json!({ ID_PARAM: job.clone(), MAX_OUTPUT_PARAM: 1 }),
                &context,
            )
            .await
            .expect("a tiny cap clamps to the floor");

        assert!(result.success, "{result:?}");
        assert!(result.output.unwrap().contains("kept"), "the floor holds");

        Jobs::kill_session(session).await;
    }
}
