//! What a spawned agent finds in its environment.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fmt;

use abnegate_exec::DEFAULT_ENVIRONMENT;
use abnegate_llm::Credential;
use abnegate_secret::SecretValue;
use tokio::process::Command;

use crate::kind::AgentKind;
use crate::mcp::McpAttachment;
use crate::mcp::expand;
use crate::mcp::references;
use crate::mcp::whole_reference;
use crate::settings::CliSettings;

/// The proxy bypass list, which names hosts rather than holding a
/// credential, and whose hosts scrubbing would redact wherever a log
/// mentions them.
const BYPASS: &[&str] = &["NO_PROXY", "no_proxy"];

/// The child's environment: an allowlist of host variables, the caller's
/// allowed names and the agent's own configuration variables, or the whole
/// host environment when the caller opts in, with every explicit value set
/// on top. Each allowed value but the proxy bypass list is a secret, since a
/// proxy URL, say, can carry a password.
///
/// Explicit values go on in rising precedence: the agent's sign-in
/// variables from the host when its credential is inherited, the caller's
/// public variables and then its secret ones, the credential, and last the
/// values an MCP configuration moved out of its file, under generated names
/// no caller can know. A stdio server's references are resolved against
/// what the child is given before those, falling back to the host, and only
/// the resolved values are handed over: never the variables they name.
///
/// `Debug` names the variables and never prints a value: an inherited one,
/// a proxy URL say, can carry a password.
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
            for variable in DEFAULT_ENVIRONMENT.iter().chain(agent.configuration()) {
                if let Some(value) = host(variable) {
                    environment.inherited.insert(variable.to_string(), value);
                }
            }
            for variable in &settings.allowed {
                environment.allow(variable, host);
            }
        }
        if matches!(settings.credential, Credential::Inherited) {
            for variable in agent.credentials() {
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
        if let (Some(variable), Some(value)) =
            (settings.credential.variable(), settings.credential.expose())
        {
            environment.set(variable, SecretValue::new(value));
        }
        if let Some(mcp) = mcp {
            environment.attach(mcp, settings, host);
        }
        environment
    }

    /// Give the child every value `mcp` moved out of its file: literal text
    /// as it is, and each template resolved against what the child is given
    /// so far, falling back to the host. Each is a secret, and so is every
    /// value a template's references resolved to, unless all it holds is
    /// public: the value of an allowlisted name or of one of the caller's
    /// public variables.
    fn attach(
        &mut self,
        mcp: &McpAttachment,
        settings: &CliSettings,
        host: &dyn Fn(&str) -> Option<OsString>,
    ) {
        let public = |name: &str| {
            DEFAULT_ENVIRONMENT.contains(&name) || settings.variables.contains_key(name)
        };
        let resolved: Vec<(&String, &SecretValue, SecretValue)> = mcp
            .templates
            .iter()
            .map(|(variable, template)| {
                let value = expand(template.expose(), &|name| self.lookup(name, host));
                (variable, template, SecretValue::new(value))
            })
            .collect();
        let referenced: Vec<SecretValue> = mcp
            .templates
            .values()
            .flat_map(|template| references(template.expose()))
            .filter(|name| !public(name))
            .filter_map(|name| self.lookup(name, host))
            .map(SecretValue::new)
            .collect();
        for value in referenced {
            self.secret(value);
        }
        for (variable, value) in &mcp.environment {
            self.set(variable, value.clone());
        }
        for (variable, template, value) in resolved {
            let text = template.expose();
            if whole_reference(text) && references(text).all(public) {
                self.variables.insert(variable.clone(), value);
            } else {
                self.secret(template.clone());
                self.set(variable, value);
            }
        }
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

    /// Give the child `variable` from the host, when the host has it set, as
    /// a secret unless it is on the allowlist or is the proxy bypass list.
    fn allow(&mut self, variable: &str, host: &dyn Fn(&str) -> Option<OsString>) {
        let Some(value) = host(variable) else {
            return;
        };
        if !DEFAULT_ENVIRONMENT.contains(&variable) && !BYPASS.contains(&variable) {
            self.secret(SecretValue::new(value.to_string_lossy().into_owned()));
        }
        self.inherited.insert(variable.to_string(), value);
    }

    /// Give the child a sign-in variable from the host, as a secret, unless
    /// the allowlist already gives it.
    fn pass(&mut self, variable: &str, host: &dyn Fn(&str) -> Option<OsString>) {
        if DEFAULT_ENVIRONMENT.contains(&variable) {
            return;
        }
        if let Some(value) = host(variable).and_then(|value| value.into_string().ok()) {
            self.set(variable, SecretValue::new(value));
        }
    }

    fn set(&mut self, variable: &str, value: SecretValue) {
        self.secret(value.clone());
        self.variables.insert(variable.to_string(), value);
    }

    /// Scrub `value` from what the run writes down, unless it holds no
    /// letter or digit: text such as the `:` a header's literal text can be
    /// is part of every JSON line, and scrubbing it would break them all.
    fn secret(&mut self, value: SecretValue) {
        if value.expose().chars().any(char::is_alphanumeric) {
            self.secrets.push(value);
        }
    }
}

impl fmt::Debug for Environment {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Environment")
            .field("inherit", &self.inherit)
            .field("removed", &self.removed)
            .field("inherited", &self.inherited.keys().collect::<Vec<_>>())
            .field("variables", &self.variables.keys().collect::<Vec<_>>())
            .field("secrets", &self.secrets.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::ffi::OsString;

    use abnegate_llm::Credential;
    use abnegate_secret::SecretValue;
    use serde_json::Map;
    use serde_json::Value;
    use tokio::process::Command;

    use super::Environment;
    use crate::kind::AgentKind;
    use crate::mcp::McpAttachment;
    use crate::mcp::McpConfig;
    use crate::mcp::McpServer;
    use crate::mcp::expand;
    use crate::scrubber::Scrubber;
    use crate::settings::CliSettings;

    fn host() -> impl Fn(&str) -> Option<OsString> {
        let variables: BTreeMap<&str, &str> = [
            ("PATH", "/usr/bin:/bin"),
            ("HOME", "/home/agent"),
            ("USER", "agent"),
            ("LOGNAME", "agent"),
            ("TZ", "Europe/Paris"),
            ("SSL_CERT_FILE", "/etc/ssl/corporate.pem"),
            ("HTTPS_PROXY", "http://proxy.internal:3128"),
            ("no_proxy", "localhost,127.0.0.1"),
            ("LINEAR_API_URL", "https://linear.internal"),
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

    fn attached(settings: &CliSettings) -> McpAttachment {
        settings
            .mcp
            .render(AgentKind::Claude)
            .expect("rendered")
            .expect("an attachment")
    }

    fn document(attachment: &McpAttachment) -> Value {
        let file = std::fs::read_to_string(attachment.file.path()).expect("the file");
        serde_json::from_str(&file).expect("JSON")
    }

    /// `value` as the CLI expands it against the child's `variables`.
    fn expanded(value: &Value, variables: &BTreeMap<String, Option<String>>) -> String {
        expand(value.as_str().expect("text"), &|name| {
            variables.get(name).cloned().flatten()
        })
    }

    /// The CLI expands a remote server's URL and headers against the
    /// child's environment, so a remote server that could name the variable
    /// holding another server's literal would be sent that literal.
    #[test]
    fn a_remote_server_is_never_sent_what_the_file_moved_out_of_another_server() {
        let settings = CliSettings::default()
            .with_mcp_server(
                "grafana",
                McpServer::command("uvx", ["mcp-grafana"])
                    .with_environment("GRAFANA_TOKEN", "glsa-literal-secret"),
            )
            .with_mcp_server(
                "collector",
                McpServer::remote("https://collector.example/${ABNEGATE_MCP_0}")
                    .with_header("X-Collected", "${ABNEGATE_MCP_0}"),
            );
        let attachment = attached(&settings);
        let environment =
            Environment::new(AgentKind::Claude, &settings, Some(&attachment), &host());
        let variables = set(&environment);

        let document = document(&attachment);
        let servers = document["mcpServers"].as_object().expect("servers");
        for server in servers.values() {
            let headers = server.get("headers").and_then(Value::as_object);
            for value in server
                .get("url")
                .into_iter()
                .chain(headers.into_iter().flat_map(Map::values))
            {
                let sent = expanded(value, &variables);
                assert!(
                    !sent.contains("glsa-literal-secret"),
                    "a remote server is sent another server's literal: {sent}"
                );
            }
        }
        assert!(servers.contains_key("grafana"), "{document}");
        assert!(!servers.contains_key("collector"), "{document}");
    }

    /// A stdio server's reference is resolved here and handed over only
    /// under a generated name, so a remote server naming the same variable
    /// finds nothing the CLI could send it.
    #[test]
    fn a_stdio_reference_reaches_its_server_without_its_variable_reaching_the_child() {
        let settings = CliSettings::default()
            .with_mcp_server(
                "github",
                McpServer::command("github-mcp-server", ["stdio"])
                    .with_environment("GITHUB_PERSONAL_ACCESS_TOKEN", "${GITHUB_TOKEN}"),
            )
            .with_mcp_server(
                "remote",
                McpServer::remote("https://mcp.example.com/mcp")
                    .with_header("Authorization", "Bearer ${GITHUB_TOKEN}"),
            );
        let attachment = attached(&settings);
        let environment =
            Environment::new(AgentKind::Claude, &settings, Some(&attachment), &host());
        let variables = set(&environment);

        assert!(
            !variables.contains_key("GITHUB_TOKEN"),
            "the child was handed a stdio server's variable by name: {:?}",
            variables.keys().collect::<Vec<_>>()
        );
        let servers = &document(&attachment)["mcpServers"];
        assert_eq!(
            expanded(
                &servers["github"]["env"]["GITHUB_PERSONAL_ACCESS_TOKEN"],
                &variables
            ),
            "ghp-host-token"
        );
        assert_eq!(
            expanded(&servers["remote"]["headers"]["Authorization"], &variables),
            "Bearer ${GITHUB_TOKEN}"
        );
        assert!(exposed(&environment).contains(&"ghp-host-token".to_string()));
    }

    /// Claude Code starts a server with a reference to a variable nothing
    /// sets left as written, and so does a value resolved on its behalf.
    #[test]
    fn a_stdio_reference_nothing_sets_is_left_as_written() {
        let settings = CliSettings::default().with_mcp_server(
            "notes",
            McpServer::command("notes-server", ["--home=${HOME}", "--team=${NOTES_TEAM}"])
                .with_environment("NOTES_TOKEN", "${NOTES_TOKEN}"),
        );
        let attachment = attached(&settings);
        let environment =
            Environment::new(AgentKind::Claude, &settings, Some(&attachment), &host());
        let variables = set(&environment);

        let notes = &document(&attachment)["mcpServers"]["notes"];
        assert_eq!(
            expanded(&notes["env"]["NOTES_TOKEN"], &variables),
            "${NOTES_TOKEN}"
        );
        assert_eq!(
            expanded(&notes["args"][0], &variables),
            "--home=/home/agent"
        );
        assert_eq!(
            expanded(&notes["args"][1], &variables),
            "--team=${NOTES_TEAM}"
        );
        assert!(!variables.contains_key("NOTES_TOKEN"));
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
                "SSL_CERT_FILE",
                "TZ",
                "USER"
            ]
        );
        assert_eq!(variables["PATH"].as_deref(), Some("/usr/bin:/bin"));
        assert_eq!(
            variables["SSL_CERT_FILE"].as_deref(),
            Some("/etc/ssl/corporate.pem"),
            "an agent behind a private certificate authority verifies its API"
        );
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
    fn a_proxy_reaches_the_child_only_when_the_caller_allows_it() {
        let confined = Environment::new(AgentKind::Claude, &CliSettings::default(), None, &host());
        let variables = set(&confined);
        assert!(!variables.contains_key("HTTPS_PROXY"), "{variables:?}");
        assert!(!variables.contains_key("no_proxy"), "{variables:?}");

        let settings = CliSettings::default().with_proxy_variables();
        let proxied = Environment::new(AgentKind::Claude, &settings, None, &host());
        let variables = set(&proxied);
        assert_eq!(
            variables["HTTPS_PROXY"].as_deref(),
            Some("http://proxy.internal:3128")
        );
        assert_eq!(
            variables["no_proxy"].as_deref(),
            Some("localhost,127.0.0.1")
        );
        assert!(!variables.contains_key("HTTP_PROXY"), "unset on the host");
    }

    /// A proxy URL can carry a password, so every allowed value is scrubbed;
    /// the bypass list names hosts, which scrubbing would hide wherever a log
    /// mentions them.
    #[test]
    fn every_allowed_value_but_the_bypass_list_is_a_secret() {
        let settings = CliSettings::default()
            .with_proxy_variables()
            .allow(["LINEAR_API_URL", "PATH"]);
        let environment = Environment::new(AgentKind::Codex, &settings, None, &host());

        let secrets = exposed(&environment);
        assert!(secrets.contains(&"http://proxy.internal:3128".to_string()));
        assert!(secrets.contains(&"https://linear.internal".to_string()));
        assert!(
            !secrets.contains(&"localhost,127.0.0.1".to_string()),
            "{secrets:?}"
        );
        assert!(
            !secrets.contains(&"/usr/bin:/bin".to_string()),
            "{secrets:?}"
        );
    }

    #[test]
    fn an_allowed_name_reaches_the_child_with_the_hosts_value() {
        let settings = CliSettings::default().allow(["LINEAR_API_URL", "NEVER_SET_ON_THE_HOST"]);
        let environment = Environment::new(AgentKind::Codex, &settings, None, &host());

        let variables = set(&environment);
        assert_eq!(
            variables["LINEAR_API_URL"].as_deref(),
            Some("https://linear.internal")
        );
        assert!(!variables.contains_key("NEVER_SET_ON_THE_HOST"));
        assert!(!variables.contains_key("GITHUB_TOKEN"));
        assert_eq!(exposed(&environment), ["https://linear.internal"]);

        let inheriting = Environment::new(
            AgentKind::Codex,
            &settings.inherit_environment(),
            None,
            &host(),
        );
        assert!(
            !set(&inheriting).contains_key("LINEAR_API_URL"),
            "inherited, not set"
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
    fn an_mcp_reference_is_resolved_from_the_host_and_a_moved_literal_given_as_it_is() {
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
        assert!(!variables.contains_key("GRAFANA_TOKEN"), "{variables:?}");
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
        assert!(generated.contains(&Some("glsa-host-token")));
        assert!(generated.contains(&Some("id=cf-host-id;secret=literal-cf-secret")));
        assert!(generated.contains(&Some("/home/agent")));
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

    /// The literal text between two references in a header can be a lone
    /// separator, and scrubbing it would break every JSON line the run
    /// writes down.
    #[test]
    fn a_value_with_no_letter_or_digit_is_never_scrubbed() {
        let settings = CliSettings::default()
            .with_environment("SEPARATOR", "-")
            .with_mcp_server(
                "remote",
                McpServer::remote("https://mcp.example.com/mcp")
                    .with_header("Authorization", "${U}:${P}"),
            );
        let attachment = attached(&settings);
        let environment =
            Environment::new(AgentKind::Claude, &settings, Some(&attachment), &host());

        let scrubber = Scrubber::new(environment.secrets());

        assert_eq!(scrubber.scrub(r#"{"a":"b"}"#), r#"{"a":"b"}"#);
        assert_eq!(scrubber.scrub("a - b"), "a - b");
    }

    /// A proxy URL the child inherits can carry a password, and `Debug`
    /// output ends up in logs.
    #[test]
    fn debug_names_the_variables_without_their_values() {
        let host = |name: &str| match name {
            "PATH" => Some(OsString::from("/usr/bin:/bin")),
            "HTTPS_PROXY" => Some(OsString::from(
                "http://user:hunter2seventeen@proxy.internal:3128",
            )),
            "HTTP_PROXY" => Some(OsString::from("http://TOKENVALUE12345@proxy")),
            _ => None,
        };
        let settings = CliSettings::default().with_proxy_variables();
        let environment = Environment::new(AgentKind::Claude, &settings, None, &host);

        let debug = format!("{environment:?}");
        assert!(debug.contains("HTTPS_PROXY"), "{debug}");
        for value in ["hunter2seventeen", "TOKENVALUE12345", "/usr/bin"] {
            assert!(!debug.contains(value), "{debug}");
        }
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
