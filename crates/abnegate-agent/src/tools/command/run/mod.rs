mod params;

pub(super) use params::RunCommandParams;

use abnegate_exec::Proxy;
use async_trait::async_trait;
use serde_json::{Value, json};
use std::process::Stdio;
use tokio::process::Command;
use tokio::time::{Duration, timeout};

use super::{
    BACKGROUND_PARAM, MAX_OUTPUT_PARAM, background, background_property, clamp_output_chars,
    max_output_property, run_preview, working_directory,
};
use crate::tools::job::JobCommand;
use crate::tools::{
    ERROR_PREFIX, REASON_PARAM, Tier, Tool, ToolContext, ToolError, ToolResult, reason_property,
    trim_middle,
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

    fn preview(&self, params: &Value) -> Option<String> {
        let params: RunCommandParams = serde_json::from_value(params.clone()).ok()?;
        let line = std::iter::once(params.command)
            .chain(params.args)
            .collect::<Vec<String>>()
            .join(" ");
        Some(run_preview(&line, params.cwd.as_deref()))
    }

    fn timeout(&self, context: &ToolContext) -> Duration {
        // Loose enough never to pre-empt the per-call limit applied below.
        Duration::from_secs(context.command_timeout + 30)
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
                BACKGROUND_PARAM: background_property(),
                MAX_OUTPUT_PARAM: max_output_property(),
                REASON_PARAM: reason_property()
            },
            "required": ["command", REASON_PARAM]
        })
    }

    async fn execute(&self, params: Value, context: &ToolContext) -> Result<ToolResult, ToolError> {
        let params: RunCommandParams = serde_json::from_value(params)
            .map_err(|error| ToolError::InvalidParams(error.to_string()))?;

        tracing::debug!(
            tool = self.name(),
            reason_given = params
                .reason
                .as_deref()
                .is_some_and(|why| !why.trim().is_empty()),
            "Running tool"
        );

        if !ALLOWED_COMMANDS.contains(&params.command.as_str()) {
            return Err(ToolError::Execution(format!(
                "Command '{}' is not in the allowed list. Name a program, not a path: cargo, npm, git, python, etc.",
                params.command
            )));
        }

        for argument in &params.args {
            if let Some(pattern) = SHELL_METACHARACTERS
                .iter()
                .find(|pattern| argument.contains(*pattern))
            {
                return Err(ToolError::Execution(format!(
                    "Argument contains potentially dangerous pattern: '{pattern}'"
                )));
            }
        }

        let cwd = working_directory(context, params.cwd.as_deref())?;

        if params.background {
            let command = JobCommand::new(&params.command, params.args.clone()).within(&cwd);
            return background(&command, context).await;
        }

        let mut process = Command::new(&params.command);
        process
            .args(&params.args)
            .current_dir(&cwd)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        process.env_clear();
        for (key, value) in &context.env {
            process.env(key, value);
        }
        Proxy::from_env().apply(&mut process);

        let timeout_duration =
            Duration::from_secs(params.timeout_secs.unwrap_or(context.command_timeout));

        let output = match timeout(timeout_duration, process.output()).await {
            Ok(result) => result
                .map_err(|error| ToolError::Execution(format!("Failed to execute: {error}")))?,
            Err(_) => {
                return Err(ToolError::Execution(format!(
                    "Command timed out after {} seconds",
                    timeout_duration.as_secs()
                )));
            }
        };

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

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

        let output_chars = clamp_output_chars(params.max_output_chars);

        if output.status.success() {
            Ok(ToolResult::success(trim_middle(&result, output_chars)))
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
                output_chars.saturating_sub(ERROR_PREFIX.chars().count()),
            )))
        }
    }
}
