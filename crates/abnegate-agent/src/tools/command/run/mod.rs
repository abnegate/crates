mod parameters;

pub(super) use parameters::RunCommandParameters;

use abnegate_exec::Proxy;
use async_trait::async_trait;
use serde_json::{Value, json};
use tokio::process::Command;
use tokio::time::Duration;

use super::{
    BACKGROUND_PARAMETER, MAX_OUTPUT_PARAMETER, background, background_property,
    clamp_output_characters, max_output_property, run_preview, working_directory,
};
use crate::tools::job::JobCommand;
use crate::tools::process;
use crate::tools::{
    ERROR_PREFIX, REASON_PARAMETER, Tier, Tool, ToolContext, ToolError, ToolResult,
    reason_property, trim_middle,
};

/// Programs [`RunCommandTool`] may spawn, resolved on the child's `PATH`.
///
/// Matched against the whole `command`, never its last path segment: an agent
/// may write a file into `cwd`, so a basename match would admit `./cargo` and
/// then run whatever that file is.
const ALLOWED_COMMANDS: &[&str] = &[
    "cargo", "rustc", "npm", "npx", "yarn", "pnpm", "node", "deno", "bun", "make", "cmake",
    "gradle", "mvn", "maven", "go", "python", "python3", "pip", "pip3", "poetry", "uv", "ruby",
    "gem", "bundle", "rake", "dotnet", "msbuild", "git", "gh", "hub", "ls", "cat", "head", "tail",
    "grep", "find", "wc", "sort", "uniq", "diff", "tree", "file", "stat", "pwd", "which",
    "whereis", "pytest", "jest", "mocha", "rspec", "phpunit", "echo", "printf", "date", "env",
    "true", "false", "test", "curl", "wget", "jq", "yq", "docker",
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

    fn preview(&self, parameters: &Value) -> Option<String> {
        let parameters: RunCommandParameters = serde_json::from_value(parameters.clone()).ok()?;
        let line = std::iter::once(parameters.command)
            .chain(parameters.arguments)
            .collect::<Vec<String>>()
            .join(" ");
        Some(run_preview(&line, parameters.working_directory.as_deref()))
    }

    fn timeout(&self, context: &ToolContext) -> Duration {
        // Loose enough never to pre-empt the per-call limit applied below.
        context.command_timeout + Duration::from_secs(30)
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
                    "description": "Timeout in seconds (default: 300)"
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
                "Command '{}' is not in the allowed list. Name a program, not a path: cargo, npm, git, python, etc.",
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

        let cwd = working_directory(context, parameters.working_directory.as_deref())?;

        if parameters.background {
            let command =
                JobCommand::new(&parameters.command, parameters.arguments.clone()).within(&cwd);
            return background(&command, context).await;
        }

        let mut process = Command::new(&parameters.command);
        process.args(&parameters.arguments).current_dir(&cwd);
        process.env_clear();
        for (key, value) in &context.env {
            process.env(key, value);
        }
        Proxy::from_env().apply(&mut process);

        let limit = parameters
            .timeout_seconds
            .map_or(context.command_timeout, Duration::from_secs);
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
