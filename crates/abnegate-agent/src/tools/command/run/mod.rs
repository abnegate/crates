mod parameters;

use std::iter::once;

use async_trait::async_trait;
pub(super) use parameters::RunCommandParameters;
use serde_json::Value;
use serde_json::json;
use tokio::time::Duration;

use super::BACKGROUND_PARAMETER;
use super::MAX_OUTPUT_PARAMETER;
use super::MAX_SHELL_TIMEOUT_SECONDS;
use super::background;
use super::background_property;
use super::call_limit;
use super::clamp_output_characters;
use super::max_output_property;
use super::run_preview;
use super::working_directory;
use crate::tools::ERROR_PREFIX;
use crate::tools::REASON_PARAMETER;
use crate::tools::TIMEOUT_SLACK;
use crate::tools::Tier;
use crate::tools::Tool;
use crate::tools::ToolContext;
use crate::tools::ToolError;
use crate::tools::ToolResult;
use crate::tools::job::JobCommand;
use crate::tools::process;
use crate::tools::reason_property;
use crate::tools::trim_middle;
use crate::tools::word;

/// Programs [`RunCommandTool`] may spawn, resolved on the child's `PATH`.
///
/// Matched against the whole `command`, never its last path segment: an agent
/// may write a file into the working directory, so a basename match would
/// admit `./cargo` and then run whatever that file is.
///
/// No interpreter (`python`, `node`, `ruby`, `deno`, `bun`, a shell), no
/// `env` and no `docker`: each of those runs whatever code its arguments
/// name, which would make the list a formality. What is left still runs
/// code from the tree - a build script, a test, a git hook - so the list
/// narrows which program starts, not what it can do.
pub(super) const ALLOWED_COMMANDS: &[&str] = &[
    "cargo", "rustc", "npm", "npx", "yarn", "pnpm", "make", "cmake", "gradle", "mvn", "maven",
    "go", "pip", "pip3", "poetry", "uv", "gem", "bundle", "rake", "dotnet", "msbuild", "git", "gh",
    "hub", "ls", "cat", "head", "tail", "grep", "find", "wc", "sort", "uniq", "diff", "tree",
    "file", "stat", "pwd", "which", "whereis", "pytest", "jest", "mocha", "rspec", "phpunit",
    "echo", "printf", "date", "true", "false", "test", "curl", "wget", "jq", "yq",
];

/// Shell syntax an argument may not carry, because an argument is handed to the
/// child whole and must never be read as a second command.
const SHELL_METACHARACTERS: &[&str] = &["$(", "`", "&&", "||", ";", "|", ">", "<", "\n", "\r"];

/// Run a program from a fixed allow-list, with no shell in between.
pub struct RunCommandTool;

#[async_trait]
impl Tool for RunCommandTool {
    fn name(&self) -> &str {
        "run_command"
    }

    fn description(&self) -> &str {
        "Execute a shell command. Returns stdout/stderr output. Use for running tests, builds, git commands, etc."
    }

    fn tier(&self) -> Tier {
        Tier::Host
    }

    /// The program and each argument as the shell word it is, blank space
    /// and all, so where one argument ends and the next begins is on the card.
    fn preview(&self, parameters: &Value) -> Option<String> {
        let parameters: RunCommandParameters = serde_json::from_value(parameters.clone()).ok()?;
        let line = once(&parameters.command)
            .chain(&parameters.arguments)
            .map(|argument| word(argument))
            .collect::<Vec<_>>()
            .join(" ");
        Some(run_preview(&line, parameters.working_directory.as_deref()))
    }

    fn timeout(&self, _context: &ToolContext) -> Duration {
        Duration::from_secs(MAX_SHELL_TIMEOUT_SECONDS) + TIMEOUT_SLACK
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The command to run (e.g., 'cargo', 'npm', 'git')"
                },
                "args": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Arguments to pass to the command"
                },
                "cwd": {
                    "type": "string",
                    "description": "Working directory for the command (relative to project root)"
                },
                "timeout_secs": {
                    "type": "integer",
                    "description": format!(
                        "Wall-clock limit in seconds. Defaults to the configured command \
                         timeout; at most {MAX_SHELL_TIMEOUT_SECONDS}."
                    )
                },
                BACKGROUND_PARAMETER: background_property(),
                MAX_OUTPUT_PARAMETER: max_output_property(),
                REASON_PARAMETER: reason_property()
            },
            "required": ["command", REASON_PARAMETER]
        })
    }

    async fn execute(
        &self,
        parameters: Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let parameters: RunCommandParameters = serde_json::from_value(parameters)
            .map_err(|error| ToolError::InvalidParameters(error.to_string()))?;

        tracing::debug!(
            tool = self.name(),
            reason_given = parameters
                .reason
                .as_deref()
                .is_some_and(|why| !why.trim().is_empty()),
            "Running tool"
        );

        if !ALLOWED_COMMANDS.contains(&parameters.command.as_str()) {
            return Err(ToolError::Execution(format!(
                "Command '{}' is not in the allowed list. Name a program, not a path: cargo, npm, git, etc.",
                parameters.command
            )));
        }

        for argument in &parameters.arguments {
            if let Some(pattern) = SHELL_METACHARACTERS
                .iter()
                .find(|pattern| argument.contains(*pattern))
            {
                return Err(ToolError::Execution(format!(
                    "Argument contains potentially dangerous pattern: '{pattern}'"
                )));
            }
        }

        let directory = working_directory(context, parameters.working_directory.as_deref())?;

        if parameters.background {
            let command = JobCommand::new(&parameters.command, parameters.arguments.clone())
                .within(&directory);
            return background(&command, context).await;
        }

        let mut process = process::command(&parameters.command, context);
        process.args(&parameters.arguments).current_dir(&directory);

        let limit = call_limit(parameters.timeout_seconds, context.command_timeout);
        let output = process::run(process, limit).await?;
        let (stdout, stderr) = (output.stdout, output.stderr);

        let mut result = String::new();

        if !stdout.is_empty() {
            result.push_str("stdout:\n");
            result.push_str(&stdout);
        }

        if !stderr.is_empty() {
            if !result.is_empty() {
                result.push_str("\n\n");
            }
            result.push_str("stderr:\n");
            result.push_str(&stderr);
        }

        if result.is_empty() {
            result = "(no output)".to_string();
        }

        let output_characters = clamp_output_characters(parameters.max_output_characters);

        if output.status.success() {
            Ok(ToolResult::success(trim_middle(&result, output_characters)))
        } else {
            let code = output
                .status
                .code()
                .map(|code| code.to_string())
                .unwrap_or_else(|| "unknown".to_string());
            // `to_message` prefixes a failure, so the cap the caller asked for
            // has to cover that too.
            Ok(ToolResult::error(trim_middle(
                &format!("Command exited with code {}\n\n{}", code, result),
                output_characters.saturating_sub(ERROR_PREFIX.chars().count()),
            )))
        }
    }
}
