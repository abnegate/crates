mod parameters;

use std::borrow::Cow;

use async_trait::async_trait;
pub(super) use parameters::RunShellParameters;
use serde_json::Value;
use serde_json::json;
use tokio::time::Duration;

use super::BACKGROUND_PARAMETER;
use super::MAXIMUM_OUTPUT_PARAMETER;
use super::MAXIMUM_SHELL_TIMEOUT;
use super::background;
use super::background_property;
use super::call_limit;
use super::clamp_output_characters;
use super::maximum_output_property;
use super::run_preview;
use super::working_directory;
use crate::tool::REASON_PARAMETER;
use crate::tool::Rendering;
use crate::tool::TIMEOUT_SLACK;
use crate::tool::Tier;
use crate::tool::Tool;
use crate::tool::ToolContext;
use crate::tool::ToolError;
use crate::tool::ToolResult;
use crate::tool::job::JobCommand;
use crate::tool::job::SHELL;
use crate::tool::job::SHELL_COMMAND_FLAG;
use crate::tool::job::WAIT_FOR;
use crate::tool::process;
use crate::tool::reason_property;
use crate::tool::trim_middle;

/// Run a command through a real shell, with no allow-list.
///
/// [`RunCommandTool`](super::RunCommandTool) spawns a binary from a fixed list and rejects shell
/// metacharacters, which rules out pipes, redirection and chaining. That is
/// the right trade for a task runner working in a checkout. This is the tool
/// for callers who have deliberately asked for an unrestricted agent: it runs
/// whatever it is given, as whoever runs the process. Register it only
/// alongside a [`ToolContext`] with `unrestricted` set.
pub struct RunShellTool;

/// Longest a call may spend blocked on `sleep`.
///
/// A run that has produced nothing for this long is announced as stalled, so a
/// longer sleep reads as a wedged run rather than a waiting one. Waiting past
/// it belongs between calls, where the loop can still see what is happening.
pub const MAXIMUM_SLEEP: Duration = Duration::from_secs(60);

/// How long a shell call runs when it names no limit of its own.
const DEFAULT_SHELL_TIMEOUT: Duration = Duration::from_secs(120);

/// Where one command in a line ends and the next begins, plus the grouping
/// characters a `sleep` can sit behind.
const COMMAND_BOUNDARIES: [char; 9] = [';', '&', '|', '\n', '(', ')', '{', '}', '`'];

/// Seconds `command` blocks on `sleep` for, adding up every `sleep` it holds.
///
/// The sleeps are summed rather than compared: `sleep 40; sleep 40` blocks for
/// eighty seconds, and a cap that only ever saw the longer of the two would
/// wave it through. A branch that will not be taken is counted too, which
/// overstates the wait rather than understating it.
///
/// Only a `sleep` in command position is visible here. One reached through a
/// script, an interpreter or a variable is left to the per-call timeout, which
/// this sits in front of rather than replaces.
pub(super) fn total_sleep(command: &str) -> Option<f64> {
    let sleeps: Vec<f64> = command
        .split(COMMAND_BOUNDARIES)
        .filter_map(|segment| {
            let mut words = segment
                .split_whitespace()
                .skip_while(|word| word.contains('='));
            let program = words.next()?.rsplit('/').next()?;
            (program == "sleep").then(|| words.map_while(sleep_seconds).sum::<f64>())
        })
        .collect();
    (!sleeps.is_empty()).then(|| sleeps.iter().sum())
}

/// One `sleep` operand in seconds: a count with an optional s, m, h or d.
///
/// A bare number is seconds and several operands add up, both as `sleep` reads
/// them. An operand that is not a duration ends the sum rather than the call:
/// what a variable holds is not knowable from here.
///
/// A negative operand counts as nothing. `sleep` rejects one rather than
/// running time backwards, and letting it subtract would have let a caller pay
/// for a long wait with a short one that never happens.
fn sleep_seconds(operand: &str) -> Option<f64> {
    let scale = match operand.chars().last()? {
        's' => 1.0,
        'm' => 60.0,
        'h' => 3_600.0,
        'd' => 86_400.0,
        _ => {
            return operand
                .parse::<f64>()
                .ok()
                .filter(|seconds| seconds.is_finite())
                .map(|seconds| seconds.max(0.0));
        }
    };
    let count = operand[..operand.len() - 1].parse::<f64>().ok()?;
    count.is_finite().then_some((count * scale).max(0.0))
}

/// Why a command that sleeps too long is refused, in words that are true of the
/// call that asked.
///
/// Backgrounding moves who waits, not what a command may do, so a detached call
/// is refused the same cap -- but telling that call to background itself is
/// advice it has already taken, and a model handed advice it has already
/// followed repeats the call until the loop's no-progress detector ends the
/// turn with nothing to show.
fn sleep_refusal(seconds: f64, backgrounded: bool) -> String {
    let (remedy, tail) = if backgrounded {
        (
            Cow::Borrowed(
                "Backgrounding does not raise the cap. Start something that finishes on its own \
                 and",
            ),
            " rather than sleeping.",
        )
    } else {
        (
            Cow::Owned(format!("Start it with {BACKGROUND_PARAMETER}: true and")),
            ".",
        )
    };
    format!(
        "This command sleeps for {seconds} seconds, and a call may block on sleep for at most \
         {}. {remedy} wait for it with {WAIT_FOR}{tail}",
        MAXIMUM_SLEEP.as_secs()
    )
}

#[async_trait]
impl Tool for RunShellTool {
    fn name(&self) -> &str {
        "run_shell"
    }

    fn description(&self) -> &str {
        "Run a shell command and return its stdout, stderr and exit code. Runs through `sh -c`, \
         so pipes, redirection and chaining work. Use for builds, tests, git and package managers."
    }

    fn tier(&self) -> Tier {
        Tier::Host
    }

    /// The directory the command runs in and the command byte for byte as
    /// `sh` reads it, blank space and blank lines included, each verbatim but
    /// for the preview's escapes. The card is already held to
    /// [`MAXIMUM_PREVIEW_CHARACTERS`](crate::tool::MAXIMUM_PREVIEW_CHARACTERS), so
    /// squeezing would shorten nothing it needs and hide what the shell reads.
    fn preview(&self, parameters: &Value) -> Option<Rendering> {
        let parameters: RunShellParameters = serde_json::from_value(parameters.clone()).ok()?;
        Some(run_preview(
            &parameters.command,
            parameters.working_directory.as_deref(),
        ))
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": format!(
                        "Shell command to run, e.g. 'cargo test 2>&1 | tail -40'. It may not \
                         block on sleep for more than {} seconds: to wait longer, start it with \
                         {BACKGROUND_PARAMETER}: true and wait for it with {WAIT_FOR}.",
                        MAXIMUM_SLEEP.as_secs()
                    )
                },
                "cwd": {
                    "type": "string",
                    "description": "Directory to run in, relative to the project root."
                },
                "timeout_secs": {
                    "type": "integer",
                    "description": "Wall-clock limit in seconds. Default 120, maximum 900."
                },
                BACKGROUND_PARAMETER: background_property(),
                MAXIMUM_OUTPUT_PARAMETER: maximum_output_property(),
                REASON_PARAMETER: reason_property()
            },
            "required": ["command", REASON_PARAMETER]
        })
    }

    fn timeout(&self, _context: &ToolContext) -> Duration {
        MAXIMUM_SHELL_TIMEOUT + TIMEOUT_SLACK
    }

    async fn execute(
        &self,
        parameters: Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let parameters: RunShellParameters = serde_json::from_value(parameters)
            .map_err(|error| ToolError::InvalidParameters(error.to_string()))?;

        tracing::debug!(
            tool = self.name(),
            reason_given = parameters
                .reason
                .as_deref()
                .is_some_and(|why| !why.trim().is_empty()),
            "Running tool"
        );

        if parameters.command.trim().is_empty() {
            return Err(ToolError::InvalidParameters("Command is empty".to_string()));
        }

        if let Some(seconds) = total_sleep(&parameters.command)
            && seconds > MAXIMUM_SLEEP.as_secs_f64()
        {
            return Err(ToolError::Execution(sleep_refusal(
                seconds,
                parameters.background,
            )));
        }

        let directory = working_directory(context, parameters.working_directory.as_deref())?;

        if parameters.background {
            let command = JobCommand::shell(&parameters.command).within(&directory);
            return background(&command, context).await;
        }

        let limit = call_limit(parameters.timeout_seconds, DEFAULT_SHELL_TIMEOUT);

        let mut process = process::command(SHELL, context);
        process
            .arg(SHELL_COMMAND_FLAG)
            .arg(&parameters.command)
            .current_dir(&directory);

        let output = process::run(process, limit).await?;
        let (stdout, stderr) = (output.stdout, output.stderr);

        let mut report = String::new();
        match output.status.code() {
            Some(0) => {}
            Some(code) => report.push_str(&format!("Exit code: {}\n", code)),
            None => report.push_str("Killed by signal\n"),
        }
        if !stdout.trim().is_empty() {
            report.push_str(&format!("stdout:\n{}\n", stdout));
        }
        if !stderr.trim().is_empty() {
            report.push_str(&format!("stderr:\n{}\n", stderr));
        }
        if report.is_empty() {
            report.push_str("(no output)");
        }

        // A non-zero exit is an observation, not a tool failure: the model
        // should read the compiler error rather than conclude the tool broke.
        Ok(ToolResult::success(trim_middle(
            report.trim_end(),
            clamp_output_characters(parameters.maximum_output_characters),
        )))
    }
}
