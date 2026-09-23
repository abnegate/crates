//! Tools that run a command, through an allow-list or through a shell.

mod run;
mod shell;
#[cfg(test)]
mod tests;

pub use run::RunCommandTool;
pub use shell::{MAX_SLEEP_SECONDS, RunShellTool};

use serde_json::{Value, json};
use std::path::PathBuf;

use super::file::{confine, resolve};
use super::job::{self, JobCommand, Jobs, WAIT_FOR};
use super::{
    MAX_PREVIEW_CHARACTERS, MAX_TOOL_OUTPUT_CHARACTERS, ToolContext, ToolError, ToolResult, excerpt,
};

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
        Some(chars) => chars.clamp(
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
/// The command is what the reader is deciding on, so it keeps the whole budget
/// and the directory is appended after it rather than put in front of it.
fn run_preview(line: &str, cwd: Option<&str>) -> String {
    let command = excerpt(line, MAX_PREVIEW_CHARACTERS);
    match cwd {
        Some(cwd) => format!("Run `{command}` in {cwd}."),
        None => format!("Run `{command}`."),
    }
}
