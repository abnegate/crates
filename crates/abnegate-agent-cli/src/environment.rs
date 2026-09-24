//! What a spawned agent finds in its environment.

use std::collections::BTreeMap;
use std::ffi::OsString;

use abnegate_llm::Credential;
use abnegate_secret::SecretValue;
use tokio::process::Command;

use crate::kind::AgentKind;
use crate::mcp::McpAttachment;
use crate::mcp::expand;
use crate::settings::CliSettings;
use crate::settings::INHERITED_VARIABLES;

/// The child's environment: an allowlist of host variables and the agent's
/// own configuration variables, or the whole host environment when the
/// caller opts in, with every explicit value set on top.
///
/// Explicit values go on in rising precedence: the agent's sign-in
/// variables from the host when its credential is inherited, host variables
/// an attached MCP server refers to, the caller's public variables and then
/// its secret ones, the values the MCP configuration moved out of its file,
/// whose generated names no caller value can shadow, and the credential
/// last.
#[derive(Debug)]
pub(crate) struct Environment {
    inherit: bool,
    removed: &'static [&'static str],
    inherited: BTreeMap<String, OsString>,
    variables: BTreeMap<String, SecretValue>,
    secrets: Vec<SecretValue>,
}

impl Environment {
    /// `host` reads one of this process's variables.
    pub(crate) fn new(
        agent: AgentKind,
        settings: &CliSettings,
        mcp: Option<&McpAttachment>,
        host: &dyn Fn(&str) -> Option<OsString>,
    ) -> Self {
        let mut environment = Self {
            inherit: settings.inherit_environment,
            removed: agent.scrubbed(),
            inherited: BTreeMap::new(),
            variables: BTreeMap::new(),
            secrets: Vec::new(),
        };
        if !environment.inherit {
            for variable in INHERITED_VARIABLES.iter().chain(agent.configuration()) {
                if let Some(value) = host(variable) {
                    environment.inherited.insert((*variable).to_string(), value);
                }
            }
        }
        if matches!(settings.credential, Credential::Inherited) {
            for variable in agent.credentials() {
                environment.pass(variable, host);
            }
        }
        if let Some(mcp) = mcp {
            for variable in &mcp.references {
                environment.pass(variable, host);
            }
        }
        for (variable, value) in &settings.variables {
            environment
                .variables
                .insert(variable.clone(), SecretValue::new(value.as_str()));
        }
        for (variable, value) in &settings.environment {
            environment.set(variable, value.clone());
        }
        if let Some(mcp) = mcp {
            for (variable, value) in &mcp.environment {
                environment.set(variable, value.clone());
            }
            let expanded: Vec<(&String, String)> = mcp
                .templates
                .iter()
                .map(|(variable, template)| {
                    (
                        variable,
                        expand(template.expose(), &|name| environment.lookup(name, host)),
                    )
                })
                .collect();
            environment.secrets.extend(mcp.templates.values().cloned());
            for (variable, value) in expanded {
                environment.set(variable, SecretValue::new(value));
            }
        }
        if let (Some(variable), Some(value)) =
            (settings.credential.variable(), settings.credential.expose())
        {
            environment.set(variable, SecretValue::new(value));
        }
        environment
    }

    /// Every value this environment holds that must never be written down.
    pub(crate) fn secrets(&self) -> impl Iterator<Item = SecretValue> + '_ {
        self.secrets.iter().cloned()
    }

    pub(crate) fn apply(&self, command: &mut Command) {
        if self.inherit {
            for variable in self.removed {
                command.env_remove(variable);
            }
        } else {
            command.env_clear();
        }
        command.envs(&self.inherited);
        for (variable, value) in &self.variables {
            command.env(variable, value.expose());
        }
    }

    /// What the child will see for `variable` so far, falling back to the
    /// host.
    fn lookup(&self, variable: &str, host: &dyn Fn(&str) -> Option<OsString>) -> Option<String> {
        if let Some(value) = self.variables.get(variable) {
            return Some(value.expose().to_string());
        }
        self.inherited
            .get(variable)
            .cloned()
            .or_else(|| host(variable))
            .and_then(|value| value.into_string().ok())
    }

    /// Give the child a host variable that is not on the allowlist, which
    /// makes its value a secret.
    fn pass(&mut self, variable: &str, host: &dyn Fn(&str) -> Option<OsString>) {
        if INHERITED_VARIABLES.contains(&variable) {
            return;
        }
        if let Some(value) = host(variable).and_then(|value| value.into_string().ok()) {
            self.set(variable, SecretValue::new(value));
        }
    }

    fn set(&mut self, variable: &str, value: SecretValue) {
        self.secrets.push(value.clone());
        self.variables.insert(variable.to_string(), value);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::ffi::OsString;

    use abnegate_llm::Credential;
    use abnegate_secret::SecretValue;
    use tokio::process::Command;

    use super::Environment;
    use crate::kind::AgentKind;
    use crate::mcp::McpConfig;
    use crate::mcp::McpServer;
    use crate::settings::CliSettings;

    fn host() -> impl Fn(&str) -> Option<OsString> {
        let variables: BTreeMap<&str, &str> = [
            ("PATH", "/usr/bin:/bin"),
            ("HOME", "/home/agent"),
            ("USER", "agent"),
            ("LOGNAME", "agent"),
            ("AWS_SECRET_ACCESS_KEY", "aws-host-secret"),
            ("GITHUB_TOKEN", "ghp-host-token"),
            ("ANTHROPIC_API_KEY", concat!("sk-ant-", "host-key")),
            ("GRAFANA_TOKEN", "glsa-host-token"),
            ("CLAUDECODE", "1"),
            ("CLAUDE_CONFIG_DIR", "/home/agent/.claude-work"),
            (
                "CLAUDE_CODE_OAUTH_TOKEN",
                concat!("sk-ant-", "oat-host-token"),
            ),
            ("CF_ID", "cf-host-id"),
        ]
        .into();
        move |name| variables.get(name).map(OsString::from)
    }

    fn set(environment: &Environment) -> BTreeMap<String, Option<String>> {
        let mut command = Command::new("true");
        environment.apply(&mut command);
        command
            .as_std()
            .get_envs()
            .map(|(name, value)| {
                (
                    name.to_string_lossy().into_owned(),
                    value.map(|value| value.to_string_lossy().into_owned()),
                )
            })
            .collect()
    }

    fn exposed(environment: &Environment) -> Vec<String> {
        environment
            .secrets()
            .map(|secret| secret.expose().to_string())
            .collect()
    }

    #[test]
    fn a_child_gets_the_allowlist_its_key_and_what_it_was_given_and_nothing_else() {
        let settings = CliSettings::default().with_environment("LINEAR_ISSUE_ID", "ENG-42");
        let environment = Environment::new(AgentKind::Claude, &settings, None, &host());

        let variables = set(&environment);
        assert_eq!(
            variables.keys().map(String::as_str).collect::<Vec<_>>(),
            [
                "ANTHROPIC_API_KEY",
                "CLAUDE_CODE_OAUTH_TOKEN",
                "CLAUDE_CONFIG_DIR",
                "HOME",
                "LINEAR_ISSUE_ID",
                "LOGNAME",
                "PATH",
                "USER"
            ]
        );
        assert_eq!(variables["PATH"].as_deref(), Some("/usr/bin:/bin"));
        assert_eq!(
            variables["USER"].as_deref(),
            Some("agent"),
            "the macOS Keychain finds a sign-in by its user"
        );
        assert_eq!(
            variables["CLAUDE_CONFIG_DIR"].as_deref(),
            Some("/home/agent/.claude-work")
        );
        assert_eq!(
            variables["ANTHROPIC_API_KEY"].as_deref(),
            Some(concat!("sk-ant-", "host-key"))
        );
        assert_eq!(
            exposed(&environment),
            [
                concat!("sk-ant-", "host-key"),
                concat!("sk-ant-", "oat-host-token"),
                "ENG-42"
            ]
        );
    }

    #[test]
    fn a_public_variable_reaches_the_child_but_is_never_a_secret() {
        let settings = CliSettings::default()
            .with_variable("DISABLE_AUTOUPDATER", "1")
            .with_variable("TOKEN", "public")
            .with_environment("TOKEN", "secret-wins");
        let environment = Environment::new(AgentKind::Codex, &settings, None, &host());

        let variables = set(&environment);
        assert_eq!(variables["DISABLE_AUTOUPDATER"].as_deref(), Some("1"));
        assert_eq!(variables["TOKEN"].as_deref(), Some("secret-wins"));
        assert_eq!(exposed(&environment), ["secret-wins"]);
    }

    #[test]
    fn an_explicit_key_replaces_the_hosts_and_is_never_inherited_for_another_agent() {
        let settings = CliSettings::default().with_credential(Credential::key(
            "ANTHROPIC_API_KEY",
            concat!("sk-ant-", "explicit"),
        ));
        let environment = Environment::new(AgentKind::Claude, &settings, None, &host());
        let variables = set(&environment);
        assert_eq!(
            variables["ANTHROPIC_API_KEY"].as_deref(),
            Some(concat!("sk-ant-", "explicit"))
        );
        assert!(!variables.contains_key("CLAUDE_CODE_OAUTH_TOKEN"));
        assert_eq!(
            variables["CLAUDE_CONFIG_DIR"].as_deref(),
            Some("/home/agent/.claude-work"),
            "an explicit key still reads the same user's configuration"
        );
        assert_eq!(exposed(&environment), [concat!("sk-ant-", "explicit")]);

        let codex = Environment::new(AgentKind::Codex, &CliSettings::default(), None, &host());
        let variables = set(&codex);
        assert!(!variables.contains_key("ANTHROPIC_API_KEY"));
        assert!(!variables.contains_key("CLAUDE_CONFIG_DIR"));
    }

    #[test]
    fn an_mcp_reference_is_given_from_the_host_and_a_moved_literal_from_the_file() {
        let settings = CliSettings::default()
            .with_mcp_server(
                "grafana",
                McpServer {
                    command: Some("uvx".to_string()),
                    arguments: vec!["--home".to_string(), "${HOME}".to_string()],
                    environment: [
                        (
                            "GRAFANA_SERVICE_ACCOUNT_TOKEN".to_string(),
                            SecretValue::new("${GRAFANA_TOKEN}"),
                        ),
                        (
                            "GRAFANA_ORG".to_string(),
                            SecretValue::new("literal-org-secret"),
                        ),
                        (
                            "GRAFANA_HEADERS".to_string(),
                            SecretValue::new("id=${CF_ID};secret=literal-cf-secret"),
                        ),
                    ]
                    .into(),
                    ..McpServer::default()
                },
            )
            .with_environment("ABNEGATE_MCP_0", "caller-shadow");
        let attachment = McpConfig::render(&settings.mcp, AgentKind::Claude)
            .expect("rendered")
            .expect("an attachment");
        let environment =
            Environment::new(AgentKind::Claude, &settings, Some(&attachment), &host());

        let variables = set(&environment);
        assert_eq!(
            variables["GRAFANA_TOKEN"].as_deref(),
            Some("glsa-host-token")
        );
        let file = std::fs::read_to_string(attachment.file.path()).expect("the file");
        assert!(!file.contains("literal-org-secret"), "{file}");
        assert!(!file.contains("literal-cf-secret"), "{file}");
        let generated: Vec<Option<&str>> = attachment
            .environment
            .keys()
            .chain(attachment.templates.keys())
            .map(|variable| variables[variable].as_deref())
            .collect();
        assert!(
            generated.contains(&Some("literal-org-secret")),
            "a caller value shadowed a moved literal: {generated:?}"
        );
        assert!(generated.contains(&Some("id=cf-host-id;secret=literal-cf-secret")));
        assert!(!variables.contains_key("GITHUB_TOKEN"));
        let secrets = exposed(&environment);
        assert!(secrets.contains(&"glsa-host-token".to_string()));
        assert!(secrets.contains(&"literal-org-secret".to_string()));
        assert!(secrets.contains(&"id=cf-host-id;secret=literal-cf-secret".to_string()));
        assert!(secrets.contains(&"cf-host-id".to_string()));
        assert!(!secrets.contains(&"/home/agent".to_string()));
    }

    #[test]
    fn an_opted_in_child_inherits_everything_less_the_nested_session_marker() {
        let settings = CliSettings::default().inherit_environment();
        let environment = Environment::new(AgentKind::Claude, &settings, None, &host());

        let variables = set(&environment);
        assert_eq!(variables.get("CLAUDECODE"), Some(&None));
        assert!(!variables.contains_key("PATH"), "inherited, not set");
        assert_eq!(
            exposed(&environment),
            [
                concat!("sk-ant-", "host-key"),
                concat!("sk-ant-", "oat-host-token")
            ]
        );

        let explicit = CliSettings::default()
            .inherit_environment()
            .with_environment("CLAUDECODE", "1");
        let environment = Environment::new(AgentKind::Claude, &explicit, None, &host());
        assert_eq!(set(&environment)["CLAUDECODE"].as_deref(), Some("1"));
    }

    async fn signed_in(command: &mut Command) -> Option<bool> {
        let output = command
            .args(["auth", "status", "--json"])
            .output()
            .await
            .expect("the installed claude CLI");
        let status: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
        status["loggedIn"].as_bool()
    }

    #[tokio::test]
    #[ignore = "runs the installed claude CLI, which must be on PATH"]
    async fn the_installed_claude_is_signed_in_under_the_allowlist_whenever_the_host_is() {
        let host = signed_in(&mut Command::new("claude")).await;

        let environment =
            Environment::new(AgentKind::Claude, &CliSettings::default(), None, &|name| {
                std::env::var_os(name)
            });
        let mut command = Command::new("claude");
        environment.apply(&mut command);
        let child = signed_in(&mut command).await;

        assert!(host.is_some(), "claude auth status printed no loggedIn");
        assert_eq!(child, host, "the allowlist changed the sign-in claude sees");
    }

    #[test]
    fn debug_never_prints_a_secret() {
        let settings = CliSettings::default().with_environment("TOKEN", "hunter2hunter2");
        let environment = Environment::new(AgentKind::Claude, &settings, None, &host());
        let debug = format!("{environment:?}");
        assert!(!debug.contains("hunter2"), "{debug}");
        assert!(!debug.contains(concat!("sk-ant-", "host-key")), "{debug}");
    }
}
