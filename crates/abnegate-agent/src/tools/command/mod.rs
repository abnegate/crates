//! Tools that run a command, through an allow-list or through a shell.

mod run;
mod shell;
#[cfg(test)]
mod tests;

use std::path::PathBuf;
use std::time::Duration;

pub use run::RunCommandTool;
use serde_json::Value;
use serde_json::json;
pub use shell::MAX_SLEEP_SECONDS;
pub use shell::RunShellTool;

use super::MAX_TOOL_OUTPUT_CHARACTERS;
use super::ToolContext;
use super::ToolError;
use super::ToolResult;
use super::file::confine;
use super::file::resolve;
use super::job;
use super::job::JobCommand;
use super::job::Jobs;
use super::job::WAIT_FOR;

pub(super) const MAX_OUTPUT_PARAMETER: &str = "max_output_chars";
const BACKGROUND_PARAMETER: &str = "background";

/// Cap on returned output, so one noisy command cannot fill the context
/// window. Spends the shared tool budget, which the transcript cap sits above,
/// so what the tool keeps is what the model is given even once the exit-code
/// line and the `Error: ` prefix are wrapped around it.
const MAX_SHELL_OUTPUT_CHARACTERS: usize = MAX_TOOL_OUTPUT_CHARACTERS;

/// Floor for a caller-supplied cap, below which neither end of the output
/// holds enough to diagnose anything.
const MIN_SHELL_OUTPUT_CHARACTERS: usize = 500;

/// Resolve `max_output_chars` against the built-in cap.
///
/// Reduce-only: a caller may spend fewer characters than the default, never
/// more, so the constant stays the ceiling on what one call can cost.
pub(super) fn clamp_output_characters(requested: Option<u64>) -> usize {
    match requested {
        Some(characters) => characters.clamp(
            MIN_SHELL_OUTPUT_CHARACTERS as u64,
            MAX_SHELL_OUTPUT_CHARACTERS as u64,
        ) as usize,
        None => MAX_SHELL_OUTPUT_CHARACTERS,
    }
}

pub(super) fn max_output_property() -> Value {
    json!({
        "type": "integer",
        // Parsed into a u64, so a negative fails the call instead of clamping.
        "minimum": 0,
        "description": format!(
            "Cap returned output at this many characters, keeping head and tail. Default \
             {MAX_SHELL_OUTPUT_CHARACTERS}; larger values clamp down, values under \
             {MIN_SHELL_OUTPUT_CHARACTERS} clamp up."
        )
    })
}

fn background_property() -> Value {
    json!({
        "type": "boolean",
        "description": format!(
            "Detach and return immediately with a job id and log path. Use for anything \
             long-running; wait for it with {WAIT_FOR} instead of blocking. A background job \
             ends with the turn that started it, or with the run. Default false."
        )
    })
}

/// Longest a single shell command may run, whatever it asks for.
pub(super) const MAX_SHELL_TIMEOUT_SECONDS: u64 = 900;

/// How long one call may run: what it asked for, or `default` when it asked
/// for nothing, held between a second and [`MAX_SHELL_TIMEOUT_SECONDS`].
pub(super) fn call_limit(requested: Option<u64>, default: Duration) -> Duration {
    requested.map_or(default, Duration::from_secs).clamp(
        Duration::from_secs(1),
        Duration::from_secs(MAX_SHELL_TIMEOUT_SECONDS),
    )
}

/// Where the command runs, resolved and confined the way every other
/// model-supplied path in these tools is.
///
/// `Path::join` alone neither normalises `..` nor resists an absolute
/// argument, so a joined directory names whatever the caller asked for. Both
/// tools and both modes resolve it here, so the foreground and the background
/// cannot drift on what a directory is allowed to be.
fn working_directory(context: &ToolContext, directory: Option<&str>) -> Result<PathBuf, ToolError> {
    let Some(directory) = directory else {
        return Ok(context.working_directory.clone());
    };
    let resolved = resolve(&context.working_directory.join(directory));
    confine(&resolved, context)?;
    Ok(resolved)
}

/// Start a detached job, and hand back the receipt the model reads it by.
///
/// Reached only once the tool's own checks have passed, so backgrounding buys
/// a command nothing the foreground would have refused it. The job is keyed to
/// the session's own working tree and the command carries the directory the
/// child runs in, so where the model pointed the command cannot move the log.
async fn background(command: &JobCommand, context: &ToolContext) -> Result<ToolResult, ToolError> {
    Jobs::spawn(command, context)
        .await
        .map(|started| ToolResult::success(job::started_text(&started)))
        .map_err(ToolError::Execution)
}

/// The command line as it will run, for an approval card.
///
/// Rendered whole: a [`Preview`](super::Preview) too long for the card is cut
/// in the middle, so the directory goes after the command, where it stays in
/// view however long the command is.
fn run_preview(line: &str, directory: Option<&str>) -> String {
    match directory {
        Some(directory) => format!("Run `{line}` in {directory}."),
        None => format!("Run `{line}`."),
    }
}
