mod allowlisted;
mod parameters;

use std::iter::once;
use std::path::Path;
use std::sync::Arc;

use allowlisted::Allowlisted;
use async_trait::async_trait;
pub(super) use parameters::RunCommandParameters;
use serde_json::Value;
use serde_json::json;
use tokio::time::Duration;

use super::BACKGROUND_PARAMETER;
use super::MAXIMUM_OUTPUT_PARAMETER;
use super::MAXIMUM_SHELL_TIMEOUT;
use super::Unwaited;
use super::Waiting;
use super::background;
use super::background_property;
use super::call_limit;
use super::clamp_output_characters;
use super::maximum_output_property;
use super::run_preview;
use super::working_directory;
use crate::tool::ERROR_PREFIX;
use crate::tool::REASON_PARAMETER;
use crate::tool::Rendering;
use crate::tool::TIMEOUT_SLACK;
use crate::tool::Tier;
use crate::tool::Tool;
use crate::tool::ToolContext;
use crate::tool::ToolError;
use crate::tool::ToolResult;
use crate::tool::WaitFor;
use crate::tool::job::JobCommand;
use crate::tool::process;
use crate::tool::reason_property;
use crate::tool::trim_middle;
use crate::tool::word;

/// Programs [`RunCommandTool`] may spawn, resolved on the child's `PATH`,
/// unless [`RunCommandTool::with_commands`] names others.
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
pub const DEFAULT_COMMANDS: &[&str] = &[
    "cargo", "rustc", "npm", "npx", "yarn", "pnpm", "make", "cmake", "gradle", "mvn", "maven",
    "go", "pip", "pip3", "poetry", "uv", "gem", "bundle", "rake", "dotnet", "msbuild", "git", "gh",
    "hub", "ls", "cat", "head", "tail", "grep", "find", "wc", "sort", "uniq", "diff", "tree",
    "file", "stat", "pwd", "which", "whereis", "pytest", "jest", "mocha", "rspec", "phpunit",
    "echo", "printf", "date", "true", "false", "test", "curl", "wget", "jq", "yq",
];

/// The programs the context's [`PublicWeb`](crate::tool::PublicWeb) gates:
/// they run only while it has no refusal. Other programs that reach the
/// network are not gated.
const PUBLIC_WEB_COMMANDS: &[&str] = &["curl", "wget"];

/// The name the model calls the tool by, however its list is configured.
const NAME: &str = "run_command";

/// What the model is told the tool does.
const DESCRIPTION: &str = "Execute a shell command. Returns stdout/stderr output. Use for running tests, builds, git commands, etc.";

/// Shell syntax an argument may not carry, because an argument is handed to the
/// child whole and must never be read as a second command.
const SHELL_METACHARACTERS: &[&str] = &["$(", "`", "&&", "||", ";", "|", ">", "<", "\n", "\r"];

/// Run a program from [`DEFAULT_COMMANDS`], with no shell in between.
pub struct RunCommandTool;

impl RunCommandTool {
    /// `run_command` spawning only `commands`, in place of
    /// [`DEFAULT_COMMANDS`], and otherwise the same tool.
    ///
    /// Each name is matched against the whole `command` a call names, so list
    /// programs, never paths. Listing an interpreter lets a call run whatever
    /// code it hands that interpreter.
    ///
    /// ```
    /// use std::sync::Arc;
    ///
    /// use abnegate_agent::Tool;
    /// use abnegate_agent::ToolRegistry;
    /// use abnegate_agent::tool::DEFAULT_COMMANDS;
    /// use abnegate_agent::tool::RunCommandTool;
    ///
    /// let commands = [DEFAULT_COMMANDS, &["python3"]].concat();
    /// let mut registry = ToolRegistry::with_defaults();
    /// registry.register(Arc::new(RunCommandTool::with_commands(&commands)));
    /// assert_eq!(registry.get("run_command").unwrap().name(), "run_command");
    /// ```
    pub fn with_commands(commands: &[&str]) -> impl Tool + use<> {
        Allowlisted::new(commands)
    }
}

#[async_trait]
impl Tool for RunCommandTool {
    fn name(&self) -> &str {
        NAME
    }

    fn description(&self) -> &str {
        DESCRIPTION
    }

    fn tier(&self) -> Tier {
        Tier::Host
    }

    /// The directory it runs in, then the program and each argument as the
    /// shell word it is, blank space and all, so where one argument ends and
    /// the next begins is on the card.
    fn preview(&self, parameters: &Value) -> Option<Rendering> {
        let parameters: RunCommandParameters = serde_json::from_value(parameters.clone()).ok()?;
        let line = once(&parameters.command)
            .chain(&parameters.arguments)
            .map(|argument| word(argument))
            .collect::<Vec<_>>()
            .join(" ");
        Some(run_preview(&line, parameters.working_directory.as_deref()))
    }

    fn timeout(&self, _context: &ToolContext) -> Duration {
        MAXIMUM_SHELL_TIMEOUT + TIMEOUT_SLACK
    }

    fn parameters_schema(&self) -> Value {
        Self::schema(WaitFor::Offered)
    }

    async fn execute(
        &self,
        parameters: Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        self.run(parameters, context, WaitFor::Offered).await
    }

    fn unwaited(&self) -> Option<Arc<dyn Tool>> {
        Some(Arc::new(Unwaited(Self)))
    }
}

#[async_trait]
impl Waiting for RunCommandTool {
    fn schema(wait_for: WaitFor) -> Value {
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
                         timeout; at most {}.",
                        MAXIMUM_SHELL_TIMEOUT.as_secs()
                    )
                },
                BACKGROUND_PARAMETER: background_property(wait_for),
                MAXIMUM_OUTPUT_PARAMETER: maximum_output_property(),
                REASON_PARAMETER: reason_property()
            },
            "required": ["command", REASON_PARAMETER]
        })
    }

    async fn run(
        &self,
        parameters: Value,
        context: &ToolContext,
        wait_for: WaitFor,
    ) -> Result<ToolResult, ToolError> {
        run_allowed(DEFAULT_COMMANDS, parameters, context, wait_for).await
    }
}

/// Run a call to `run_command` that may spawn only what `commands` lists.
async fn run_allowed(
    commands: &[impl AsRef<str>],
    parameters: Value,
    context: &ToolContext,
    wait_for: WaitFor,
) -> Result<ToolResult, ToolError> {
    let parameters: RunCommandParameters = serde_json::from_value(parameters)
        .map_err(|error| ToolError::InvalidParameters(error.to_string()))?;

    tracing::debug!(
        tool = NAME,
        reason_given = parameters
            .reason
            .as_deref()
            .is_some_and(|why| !why.trim().is_empty()),
        "Running tool"
    );

    if !commands
        .iter()
        .any(|command| command.as_ref() == parameters.command)
    {
        return Err(ToolError::Execution(format!(
            "Command '{}' is not in the allowed list. Name a program, not a path: cargo, npm, git, etc.",
            parameters.command
        )));
    }

    let program = Path::new(&parameters.command)
        .file_name()
        .and_then(|name| name.to_str());
    if program.is_some_and(|program| PUBLIC_WEB_COMMANDS.contains(&program))
        && let Some(refusal) = context.public_web_refusal()
    {
        return Err(ToolError::Execution(refusal));
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
        let command =
            JobCommand::new(&parameters.command, parameters.arguments.clone()).within(&directory);
        return background(&command, context, wait_for).await;
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

    let output_characters = clamp_output_characters(parameters.maximum_output_characters);

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
