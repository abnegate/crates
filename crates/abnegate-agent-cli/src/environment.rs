//! What a spawned agent finds in its environment.

use std::collections::BTreeMap;
use std::ffi::OsString;

use abnegate_llm::Credential;
use abnegate_secret::SecretValue;
use tokio::process::Command;

use crate::kind::AgentKind;
use crate::mcp::McpAttachment;
use crate::settings::CliSettings;
use crate::settings::INHERITED_VARIABLES;

/// The child's environment: an allowlist of host variables, or the whole
/// host environment when the caller opts in, with every explicit value set
/// on top.
///
/// Explicit values go on in rising precedence: host variables an attached
/// MCP server refers to, the values its configuration moved out of its file,
/// the caller's own, and the credential last.
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
            for variable in INHERITED_VARIABLES {
                if let Some(value) = host(variable) {
                    environment.inherited.insert(variable.to_string(), value);
                }
            }
        }
        if matches!(settings.credential, Credential::Inherited) {
            environment.pass(agent.variable(), host);
        }
        if let Some(mcp) = mcp {
            for variable in &mcp.references {
                environment.pass(variable, host);
            }
            for (variable, value) in &mcp.environment {
                environment.set(variable, value.clone());
            }
        }
        for (variable, value) in &settings.environment {
            environment.set(variable, value.clone());
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
            ("AWS_SECRET_ACCESS_KEY", "aws-host-secret"),
            ("GITHUB_TOKEN", "ghp-host-token"),
            ("ANTHROPIC_API_KEY", concat!("sk-ant-", "host-key")),
            ("GRAFANA_TOKEN", "glsa-host-token"),
            ("CLAUDECODE", "1"),
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
            ["ANTHROPIC_API_KEY", "HOME", "LINEAR_ISSUE_ID", "PATH"]
        );
        assert_eq!(variables["PATH"].as_deref(), Some("/usr/bin:/bin"));
        assert_eq!(
            variables["ANTHROPIC_API_KEY"].as_deref(),
            Some(concat!("sk-ant-", "host-key"))
        );
        assert_eq!(
            exposed(&environment),
            [concat!("sk-ant-", "host-key"), "ENG-42"]
        );
    }

    #[test]
    fn an_explicit_key_replaces_the_hosts_and_is_never_inherited_for_another_agent() {
        let settings = CliSettings::default().with_credential(Credential::key(
            "ANTHROPIC_API_KEY",
            concat!("sk-ant-", "explicit"),
        ));
        let environment = Environment::new(AgentKind::Claude, &settings, None, &host());
        assert_eq!(
            set(&environment)["ANTHROPIC_API_KEY"].as_deref(),
            Some(concat!("sk-ant-", "explicit"))
        );
        assert_eq!(exposed(&environment), [concat!("sk-ant-", "explicit")]);

        let codex = Environment::new(AgentKind::Codex, &CliSettings::default(), None, &host());
        assert!(!set(&codex).contains_key("ANTHROPIC_API_KEY"));
    }

    #[test]
    fn an_mcp_reference_is_given_from_the_host_and_a_moved_literal_from_the_file() {
        let settings = CliSettings::default().with_mcp_server(
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
                ]
                .into(),
                ..McpServer::default()
            },
        );
        let attachment = McpConfig::render(&settings.mcp)
            .expect("rendered")
            .expect("an attachment");
        let environment =
            Environment::new(AgentKind::Claude, &settings, Some(&attachment), &host());

        let variables = set(&environment);
        assert_eq!(
            variables["GRAFANA_TOKEN"].as_deref(),
            Some("glsa-host-token")
        );
        assert_eq!(
            variables["ABNEGATE_MCP_0"].as_deref(),
            Some("literal-org-secret")
        );
        assert!(!variables.contains_key("GITHUB_TOKEN"));
        let secrets = exposed(&environment);
        assert!(secrets.contains(&"glsa-host-token".to_string()));
        assert!(secrets.contains(&"literal-org-secret".to_string()));
        assert!(!secrets.contains(&"/home/agent".to_string()));
    }

    #[test]
    fn an_opted_in_child_inherits_everything_less_the_nested_session_marker() {
        let settings = CliSettings::default().inherit_environment();
        let environment = Environment::new(AgentKind::Claude, &settings, None, &host());

        let variables = set(&environment);
        assert_eq!(variables.get("CLAUDECODE"), Some(&None));
        assert!(!variables.contains_key("PATH"), "inherited, not set");
        assert_eq!(exposed(&environment), [concat!("sk-ant-", "host-key")]);

        let explicit = CliSettings::default()
            .inherit_environment()
            .with_environment("CLAUDECODE", "1");
        let environment = Environment::new(AgentKind::Claude, &explicit, None, &host());
        assert_eq!(set(&environment)["CLAUDECODE"].as_deref(), Some("1"));
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
