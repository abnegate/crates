//! The coding agent CLIs this crate knows how to drive.

use std::fmt;
use std::path::Path;

use abnegate_llm::Capabilities;
use abnegate_llm::ProviderError;
use serde::Deserialize;
use serde::Serialize;

use crate::delivery::Delivery;
use crate::event::AgentEvent;
use crate::parser;
use crate::settings::CliSettings;
use crate::settings::WRITE_TOOLS;

const MODEL: &str = "--model";
const MCP_CONFIG: &str = "--mcp-config";
const STRICT_MCP_CONFIG: &str = "--strict-mcp-config";
const JSON_SCHEMA: &str = "--json-schema";
const APPEND_SYSTEM_PROMPT: &str = "--append-system-prompt";
const ALLOWED_TOOLS: &str = "--allowedTools";
const DISALLOWED_TOOLS: &str = "--disallowedTools";

/// Flags that let an agent act without asking, which a read-only run refuses.
const BYPASSES: [&str; 3] = [
    "--dangerously-skip-permissions",
    "--allow-dangerously-skip-permissions",
    "--permission-mode",
];

/// A coding agent CLI.
///
/// Every agent here takes its prompt on stdin. `argv` has a hard size limit
/// that a conversation reaches long before a context window does, and the
/// failure mode when it does is `E2BIG` from `execve` rather than anything the
/// agent can report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum AgentKind {
    Claude,
    Codex,
}

impl AgentKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }

    /// The command looked up on `PATH` unless the settings override it.
    pub fn executable(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }

    pub fn delivery(self) -> Delivery {
        match self {
            Self::Claude | Self::Codex => Delivery::Stdin,
        }
    }

    /// The environment variable this agent reads its key from, when a key is
    /// configured at all. An agent already signed in on the host needs none.
    pub fn variable(self) -> &'static str {
        match self {
            Self::Claude => "ANTHROPIC_API_KEY",
            Self::Codex => "OPENAI_API_KEY",
        }
    }

    /// Variables the agent sets for the commands it runs, which make a copy
    /// of it started from inside one of those commands refuse to run or
    /// behave as a nested session. They are removed from what the child
    /// inherits, before any the caller sets explicitly.
    pub fn scrubbed(self) -> &'static [&'static str] {
        match self {
            Self::Claude => &["CLAUDECODE"],
            Self::Codex => &[],
        }
    }

    /// What this agent can do beyond returning prose, as driven by this crate.
    pub fn capabilities(self) -> Capabilities {
        match self {
            Self::Claude => Capabilities::ALL,
            Self::Codex => Capabilities {
                streaming_events: true,
                ..Capabilities::NONE
            },
        }
    }

    /// A non-interactive invocation that streams newline-delimited JSON.
    ///
    /// No permission-bypass flag is passed to any agent. These commands run
    /// with the user's own credentials and file access, so the agent's own
    /// approval behaviour is left exactly as the user configured it.
    pub fn arguments(self, model: Option<&str>) -> Vec<String> {
        self.invocation(model, Vec::new())
    }

    /// [`AgentKind::arguments`] with `options` placed after the streaming flags
    /// and before the model and the flag that makes the agent read its prompt.
    pub fn invocation(self, model: Option<&str>, options: Vec<String>) -> Vec<String> {
        let mut arguments: Vec<String> = self
            .streaming()
            .iter()
            .map(|argument| (*argument).to_string())
            .collect();
        arguments.extend(options);

        if let Some(model) = model.map(str::trim).filter(|model| !model.is_empty()) {
            arguments.push(MODEL.to_string());
            arguments.push(model.to_string());
        }

        arguments.push(self.prompt().to_string());
        arguments
    }

    /// What `settings` asks of this agent, in its own flags, for
    /// [`AgentKind::invocation`]. `mcp` is the rendered MCP configuration when
    /// any server attaches, and its servers' tools join the allowed set.
    ///
    /// A setting this agent has no flag for is refused rather than dropped,
    /// since a run that silently ignored its tool restrictions or its answer
    /// schema would not be the run the caller asked for.
    pub fn options(
        self,
        settings: &CliSettings,
        mcp: Option<&Path>,
    ) -> Result<Vec<String>, ProviderError> {
        match self {
            Self::Claude => claude_options(settings, mcp),
            Self::Codex => {
                let unsupported = [
                    (settings.schema.is_some(), JSON_SCHEMA),
                    (settings.instructions.is_some(), APPEND_SYSTEM_PROMPT),
                    (!settings.permissions.is_empty(), ALLOWED_TOOLS),
                    (settings.read_only, DISALLOWED_TOOLS),
                    (!settings.mcp.is_empty(), MCP_CONFIG),
                ]
                .into_iter()
                .find_map(|(requested, flag)| requested.then_some(flag));
                match unsupported {
                    Some(flag) => Err(ProviderError::unsupported(format!(
                        "{self} has no equivalent of claude's {flag}"
                    ))),
                    None => Ok(settings.arguments.clone()),
                }
            }
        }
    }

    /// Translate one output line, appending whatever it means.
    ///
    /// A line this agent has no opinion about appends nothing rather than
    /// failing: agents add event types between releases, and a stream that
    /// aborted on the first unrecognised line would lose the whole answer.
    pub fn interpret(self, line: &str, events: &mut Vec<AgentEvent>) {
        match self {
            Self::Claude => parser::claude::interpret(line, events),
            Self::Codex => parser::codex::interpret(line, events),
        }
    }

    fn streaming(self) -> &'static [&'static str] {
        match self {
            Self::Claude => &["--verbose", "--output-format", "stream-json"],
            Self::Codex => &["exec", "--json", "--skip-git-repo-check"],
        }
    }

    fn prompt(self) -> &'static str {
        match self {
            Self::Claude => "--print",
            Self::Codex => "-",
        }
    }
}

fn claude_options(
    settings: &CliSettings,
    mcp: Option<&Path>,
) -> Result<Vec<String>, ProviderError> {
    if settings.read_only
        && let Some(bypass) = settings.arguments.iter().find(|argument| {
            BYPASSES.iter().any(|bypass| {
                argument.as_str() == *bypass
                    || argument
                        .strip_prefix(bypass)
                        .is_some_and(|rest| rest.starts_with('='))
            })
        })
    {
        return Err(ProviderError::config(format!(
            "a read-only run cannot pass {bypass}"
        )));
    }

    let mut options = Vec::new();
    if let Some(path) = mcp {
        options.push(MCP_CONFIG.to_string());
        options.push(path.display().to_string());
        options.push(STRICT_MCP_CONFIG.to_string());
    }
    if let Some(schema) = &settings.schema {
        options.push(JSON_SCHEMA.to_string());
        options.push(schema.clone());
    }
    options.extend(settings.arguments.iter().cloned());
    if let Some(instructions) = &settings.instructions {
        options.push(APPEND_SYSTEM_PROMPT.to_string());
        options.push(instructions.clone());
    }

    let mut tools: Vec<String> = Vec::new();
    let attached = mcp
        .map(|_| settings.mcp.allowed_tools())
        .unwrap_or_default();
    for tool in settings.permissions.iter().chain(&attached) {
        if !tools.contains(tool) {
            tools.push(tool.clone());
        }
    }
    for tool in tools {
        options.push(ALLOWED_TOOLS.to_string());
        options.push(tool);
    }
    if settings.read_only {
        for tool in WRITE_TOOLS {
            options.push(DISALLOWED_TOOLS.to_string());
            options.push(tool.to_string());
        }
    }
    Ok(options)
}

impl fmt::Display for AgentKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use abnegate_llm::Capabilities;
    use abnegate_llm::ProviderError;

    use super::AgentKind;
    use crate::delivery::Delivery;
    use crate::mcp::McpServer;
    use crate::settings::CliSettings;

    #[test]
    fn the_prompt_is_never_placed_on_the_command_line() {
        for agent in [AgentKind::Claude, AgentKind::Codex] {
            assert_eq!(agent.delivery(), Delivery::Stdin, "{agent}");
        }
    }

    #[test]
    fn claude_streams_json_and_reads_its_prompt_from_stdin() {
        let arguments = AgentKind::Claude.arguments(Some("opus"));

        assert_eq!(
            arguments,
            [
                "--verbose",
                "--output-format",
                "stream-json",
                "--model",
                "opus",
                "--print",
            ]
        );
    }

    #[test]
    fn codex_streams_json_and_reads_its_prompt_from_stdin() {
        let arguments = AgentKind::Codex.arguments(Some("o3"));

        assert_eq!(
            arguments,
            [
                "exec",
                "--json",
                "--skip-git-repo-check",
                "--model",
                "o3",
                "-"
            ]
        );
    }

    #[test]
    fn options_sit_between_the_streaming_flags_and_the_model() {
        let arguments = AgentKind::Claude.invocation(
            Some("sonnet"),
            vec!["--allowedTools".to_string(), "Read".to_string()],
        );

        assert_eq!(
            arguments,
            [
                "--verbose",
                "--output-format",
                "stream-json",
                "--allowedTools",
                "Read",
                "--model",
                "sonnet",
                "--print",
            ]
        );
    }

    #[test]
    fn an_absent_or_empty_model_is_left_to_the_agent() {
        for model in [None, Some(""), Some("   ")] {
            let arguments = AgentKind::Claude.arguments(model);
            assert!(
                !arguments.contains(&"--model".to_string()),
                "sent --model for {model:?}"
            );
        }
    }

    #[test]
    fn no_agent_is_ever_asked_to_skip_its_permission_prompts() {
        for agent in [AgentKind::Claude, AgentKind::Codex] {
            let arguments = AgentKind::arguments(agent, Some("model")).join(" ");
            for bypass in [
                "--dangerously-skip-permissions",
                "--full-auto",
                "--always-approve",
                "--yolo",
                "--permission-mode",
            ] {
                assert!(
                    !arguments.contains(bypass),
                    "{agent} was handed {bypass}: {arguments}"
                );
            }
        }
    }

    #[test]
    fn an_unrecognised_line_is_ignored_rather_than_fatal() {
        for agent in [AgentKind::Claude, AgentKind::Codex] {
            let mut events = Vec::new();
            agent.interpret(r#"{"type":"something_added_next_release"}"#, &mut events);
            agent.interpret("not json at all", &mut events);
            agent.interpret("", &mut events);
            assert!(events.is_empty(), "{agent} reacted to noise");
        }
    }

    #[test]
    fn a_nested_claude_session_marker_is_scrubbed_and_codex_keeps_everything() {
        assert_eq!(AgentKind::Claude.scrubbed(), ["CLAUDECODE"]);
        assert!(AgentKind::Codex.scrubbed().is_empty());
    }

    #[test]
    fn claude_supports_everything_and_codex_only_streams() {
        assert_eq!(AgentKind::Claude.capabilities(), Capabilities::ALL);

        let codex = AgentKind::Codex.capabilities();
        assert!(codex.streaming_events);
        assert!(!codex.structured_output);
        assert!(!codex.tool_permissions);
        assert!(!codex.custom_instructions);
        assert!(!codex.cost_reporting);
    }

    #[test]
    fn an_agent_reads_and_writes_its_own_name_in_configuration() {
        for agent in [AgentKind::Claude, AgentKind::Codex] {
            let json = serde_json::to_value(agent).expect("serialisable");
            assert_eq!(json, serde_json::json!(agent.as_str()));
            assert_eq!(
                serde_json::from_value::<AgentKind>(json).expect("deserialisable"),
                agent
            );
        }
    }

    #[test]
    fn each_agent_reads_its_key_from_its_own_vendor_variable() {
        assert_eq!(AgentKind::Claude.variable(), "ANTHROPIC_API_KEY");
        assert_eq!(AgentKind::Codex.variable(), "OPENAI_API_KEY");
        assert_eq!(AgentKind::Claude.executable(), "claude");
        assert_eq!(AgentKind::Codex.to_string(), "codex");
    }

    #[test]
    fn default_settings_add_no_options() {
        for agent in [AgentKind::Claude, AgentKind::Codex] {
            let options = agent
                .options(&CliSettings::default(), None)
                .expect("options");
            assert!(options.is_empty(), "{agent}: {options:?}");
        }
    }

    #[test]
    fn claude_renders_every_setting_as_its_own_flag() {
        let settings = CliSettings::default()
            .with_schema(r#"{"type":"object"}"#)
            .with_arguments(["--dangerously-skip-permissions"])
            .with_instructions("Be terse.")
            .with_permissions(["Read", "Grep"]);

        let options = AgentKind::Claude.options(&settings, None).expect("options");

        assert_eq!(
            options,
            [
                "--json-schema",
                r#"{"type":"object"}"#,
                "--dangerously-skip-permissions",
                "--append-system-prompt",
                "Be terse.",
                "--allowedTools",
                "Read",
                "--allowedTools",
                "Grep",
            ]
        );
    }

    #[test]
    fn an_attached_mcp_config_is_loaded_strictly_and_its_tools_allowed_once() {
        let server = McpServer {
            command: Some("uvx".to_string()),
            ..McpServer::default()
        };
        let settings = CliSettings::default()
            .with_permissions(["Read", "mcp__appwrite"])
            .with_mcp_server("appwrite", server.clone())
            .with_mcp_server(
                "grafana",
                McpServer {
                    tools: vec!["query".to_string()],
                    ..server
                },
            );

        let options = AgentKind::Claude
            .options(&settings, Some(Path::new("/tmp/mcp-1.json")))
            .expect("options");

        assert_eq!(
            options,
            [
                "--mcp-config",
                "/tmp/mcp-1.json",
                "--strict-mcp-config",
                "--allowedTools",
                "Read",
                "--allowedTools",
                "mcp__appwrite",
                "--allowedTools",
                "mcp__grafana__query",
            ]
        );
    }

    #[test]
    fn mcp_tools_are_not_allowed_when_no_config_attached() {
        let settings = CliSettings::default().with_mcp_server(
            "appwrite",
            McpServer {
                command: Some("uvx".to_string()),
                ..McpServer::default()
            },
        );

        let options = AgentKind::Claude.options(&settings, None).expect("options");
        assert!(options.is_empty(), "{options:?}");
    }

    #[test]
    fn a_read_only_run_denies_the_write_tools() {
        let options = AgentKind::Claude
            .options(&CliSettings::default().read_only(), None)
            .expect("options");

        let denied: Vec<&str> = options
            .windows(2)
            .filter(|pair| pair[0] == "--disallowedTools")
            .map(|pair| pair[1].as_str())
            .collect();
        assert_eq!(
            denied,
            ["Bash", "Edit", "MultiEdit", "Write", "NotebookEdit"]
        );
        let allowed: Vec<&str> = options
            .windows(2)
            .filter(|pair| pair[0] == "--allowedTools")
            .map(|pair| pair[1].as_str())
            .collect();
        assert_eq!(allowed, ["Read", "Grep", "Glob", "WebFetch", "WebSearch"]);
    }

    #[test]
    fn a_read_only_run_refuses_every_permission_bypass() {
        for bypass in [
            vec!["--dangerously-skip-permissions"],
            vec!["--allow-dangerously-skip-permissions"],
            vec!["--permission-mode", "bypassPermissions"],
            vec!["--permission-mode=acceptEdits"],
        ] {
            let settings = CliSettings::default()
                .read_only()
                .with_arguments(bypass.clone());
            let error = AgentKind::Claude
                .options(&settings, None)
                .expect_err("a refusal");
            assert!(
                matches!(error, ProviderError::Config { ref detail } if detail.contains(bypass[0].split('=').next().unwrap_or_default())),
                "{bypass:?}: {error:?}"
            );
        }
    }

    #[test]
    fn a_read_only_run_keeps_harmless_extra_arguments() {
        let settings = CliSettings::default()
            .read_only()
            .with_arguments(["--permission-modes-are-not-this-flag"]);

        let options = AgentKind::Claude.options(&settings, None).expect("options");
        assert_eq!(options[0], "--permission-modes-are-not-this-flag");
    }

    #[test]
    fn codex_passes_extra_arguments_through() {
        let settings = CliSettings::default().with_arguments(["--sandbox", "read-only"]);

        assert_eq!(
            AgentKind::Codex.options(&settings, None).expect("options"),
            ["--sandbox", "read-only"]
        );
    }

    #[test]
    fn codex_refuses_what_only_claude_can_do() {
        for (settings, flag) in [
            (CliSettings::default().with_schema("{}"), "--json-schema"),
            (
                CliSettings::default().with_instructions("Be terse."),
                "--append-system-prompt",
            ),
            (CliSettings::default().read_only(), "--allowedTools"),
            (
                CliSettings {
                    read_only: true,
                    ..CliSettings::default()
                },
                "--disallowedTools",
            ),
            (
                CliSettings::default().with_mcp_server("appwrite", McpServer::default()),
                "--mcp-config",
            ),
        ] {
            let error = AgentKind::Codex
                .options(&settings, None)
                .expect_err("a refusal");
            assert!(
                matches!(error, ProviderError::Unsupported { ref detail } if detail.contains(flag)),
                "{flag}: {error:?}"
            );
            assert!(!error.recoverable());
        }
    }
}
