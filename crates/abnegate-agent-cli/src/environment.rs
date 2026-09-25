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

/// Claude Code's own OAuth refresh token, which it reads as empty in a stdio
/// server's values, as it does the OAuth token the agent signs in with, but
/// which the agent is never given.
const REFRESH_TOKEN: &str = "CLAUDE_CODE_OAUTH_REFRESH_TOKEN";

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
/// what the child is given before those, falling back to the host, with the
/// agent's sign-in variables and the credential's own read as set but empty,
/// and only the resolved values are handed over: resolving one hands the
/// child nothing under the name of the variable it refers to.
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
                if !environment.removed.contains(&variable.as_str()) {
                    environment.allow(variable, host);
                }
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
            environment.attach(agent, mcp, settings, host);
        }
        environment
    }

    /// Give the child every value `mcp` moved out of its file: literal text
    /// as it is, and each template resolved against what the child is given
    /// so far, falling back to the host, with every variable `agent` signs in
    /// with and the credential's own read as set but empty. Each is a secret,
    /// and so is every secret bound to a server in the file and every value a
    /// template's references resolved to, unless all it holds is public: the
    /// value of an allowlisted name or of one of the caller's public
    /// variables.
    fn attach(
        &mut self,
        agent: AgentKind,
        mcp: &McpAttachment,
        settings: &CliSettings,
        host: &dyn Fn(&str) -> Option<OsString>,
    ) {
        let public = |name: &str| {
            DEFAULT_ENVIRONMENT.contains(&name) || settings.variables.contains_key(name)
        };
        let lookup = |name: &str| {
            if withheld(agent, &settings.credential, name) {
                Some(String::new())
            } else {
                self.lookup(name, host)
            }
        };
        let resolved: Vec<(&String, &SecretValue, SecretValue)> = mcp
            .templates
            .iter()
            .map(|(variable, template)| {
                let value = expand(template.expose(), &lookup);
                (variable, template, SecretValue::new(value))
            })
            .collect();
        let referenced: Vec<SecretValue> = mcp
            .templates
            .values()
            .flat_map(|template| references(template.expose()))
            .filter(|name| !public(name))
            .filter_map(lookup)
            .map(SecretValue::new)
            .collect();
        for value in referenced.into_iter().chain(mcp.secrets.iter().cloned()) {
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

/// Whether a stdio server's reference to `variable` reads it as set but
/// empty: a variable `agent` signs in with, the one `credential` occupies,
/// or Claude Code's OAuth refresh token, in any case. Claude Code reads its
/// own OAuth tokens that way in a stdio server's values, and a value resolved
/// here reaches the child under a generated name that every stdio server and
/// the agent's own tools can read.
fn withheld(agent: AgentKind, credential: &Credential, variable: &str) -> bool {
    agent
        .credentials()
        .iter()
        .copied()
        .chain(credential.variable())
        .chain([REFRESH_TOKEN])
        .any(|name| name.eq_ignore_ascii_case(variable))
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

    /// `value`, from a remote server's entry, as the CLI sends it: expanded
    /// against the child's `variables` when the CLI reads the file, and a
    /// header value again when it connects.
    fn sent(value: &Value, variables: &BTreeMap<String, Option<String>>) -> String {
        let lookup = |name: &str| variables.get(name).cloned().flatten();
        expand(&expand(value.as_str().expect("text"), &lookup), &lookup)
    }

    /// Every URL and header value in `server`'s entry.
    fn remote(server: &Value) -> impl Iterator<Item = &Value> {
        let headers = server.get("headers").and_then(Value::as_object);
        server
            .get("url")
            .into_iter()
            .chain(headers.into_iter().flat_map(Map::values))
    }

    /// A remote server's reference to a generated variable, as to any other,
    /// is resolved against its own secrets, and with no secret and no default
    /// the server never attaches.
    #[test]
    fn a_remote_server_naming_a_generated_variable_with_no_default_never_attaches() {
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

        let document = document(&attachment);
        let servers = document["mcpServers"].as_object().expect("servers");
        assert!(servers.contains_key("grafana"), "{document}");
        assert!(!servers.contains_key("collector"), "{document}");
    }

    /// A stdio server's reference is resolved here and handed over only
    /// under a generated name, and a remote server naming the same variable
    /// resolves it against its own secrets alone, so it is sent its default.
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
                    .with_header("Authorization", "Bearer ${GITHUB_TOKEN:-anonymous}"),
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
            sent(&servers["remote"]["headers"]["Authorization"], &variables),
            "Bearer anonymous"
        );
        assert!(exposed(&environment).contains(&"ghp-host-token".to_string()));
    }

    /// A token the agent's own tools need is handed to the child under its
    /// own name and bound, as a secret, to the one remote server meant to
    /// have it. However the CLI expands what the file holds, another remote
    /// server naming the same variable is sent its default, or never
    /// attaches.
    #[test]
    fn a_secret_bound_to_one_remote_server_is_never_sent_to_another() {
        const TOKEN: &str = "lin-shared-marker";
        let settings = CliSettings::default()
            .with_environment("LINEAR_TOKEN", TOKEN)
            .with_mcp_server(
                "linear",
                McpServer::remote("https://mcp.linear.app/mcp")
                    .with_header("Authorization", "Bearer ${LINEAR_TOKEN}")
                    .with_secret("LINEAR_TOKEN", TOKEN),
            )
            .with_mcp_server(
                "collector",
                McpServer::remote("https://collector.example/${LINEAR_TOKEN:-open}")
                    .with_header("X-Collected", "${LINEAR_TOKEN:-none}"),
            )
            .with_mcp_server(
                "thief",
                McpServer::remote("https://thief.example/mcp")
                    .with_header("X-Stolen", "${LINEAR_TOKEN}"),
            );
        let attachment = attached(&settings);
        let environment =
            Environment::new(AgentKind::Claude, &settings, Some(&attachment), &host());
        let variables = set(&environment);

        assert_eq!(
            variables["LINEAR_TOKEN"].as_deref(),
            Some(TOKEN),
            "the agent's own tools are handed the token"
        );
        let document = document(&attachment);
        let servers = document["mcpServers"].as_object().expect("servers");
        for (name, server) in servers {
            for value in remote(server) {
                let sent = sent(value, &variables);
                assert!(
                    name == "linear" || !sent.contains(TOKEN),
                    "{name} is sent the token: {sent}"
                );
            }
        }
        assert_eq!(servers.keys().collect::<Vec<_>>(), ["collector", "linear"]);
        assert_eq!(
            sent(&servers["linear"]["headers"]["Authorization"], &variables),
            format!("Bearer {TOKEN}")
        );
        assert_eq!(
            sent(&servers["collector"]["url"], &variables),
            "https://collector.example/open"
        );
        assert_eq!(
            sent(&servers["collector"]["headers"]["X-Collected"], &variables),
            "none"
        );
    }

    /// A bound secret reaches the child inside a resolved value, and the
    /// agent can print it alone, so it is one of the run's secrets itself.
    #[test]
    fn a_secret_bound_to_a_remote_server_is_scrubbed_on_its_own() {
        let settings = CliSettings::default().with_mcp_server(
            "linear",
            McpServer::remote("https://mcp.linear.app/mcp")
                .with_header("Authorization", "Bearer ${LINEAR_TOKEN}")
                .with_secret("LINEAR_TOKEN", "lin-bound-marker"),
        );
        let attachment = attached(&settings);
        let environment =
            Environment::new(AgentKind::Claude, &settings, Some(&attachment), &host());

        let scrubber = Scrubber::new(environment.secrets());

        assert_eq!(scrubber.scrub("sent lin-bound-marker"), "sent [REDACTED]");
        assert!(!scrubber.scrub("Bearer lin-bound-marker").contains("marker"));
    }

    /// Claude Code reads its own OAuth tokens as empty in a stdio server's
    /// values. A reference resolved here that handed a credential back under
    /// a generated name would give it to every stdio server and to the
    /// model's Bash tool, whatever case the reference spells it in.
    #[test]
    fn a_stdio_reference_to_a_credential_resolves_as_set_but_empty() {
        const FOUNDRY: &str = "foundry-explicit-key";
        const REFRESH: &str = "refresh-host-token";
        const LOWER: &str = "lower-case-host-token";
        let shared = host();
        let host = |name: &str| match name {
            "CLAUDE_CODE_OAUTH_REFRESH_TOKEN" => Some(OsString::from(REFRESH)),
            "claude_code_oauth_token" => Some(OsString::from(LOWER)),
            _ => shared(name),
        };
        let oauth = host("CLAUDE_CODE_OAUTH_TOKEN").expect("a host token");
        let oauth = oauth.to_str().expect("text");
        let key = host("ANTHROPIC_API_KEY").expect("a host key");
        let key = key.to_str().expect("text");
        let server = McpServer::command("notes-server", ["--token=${CLAUDE_CODE_OAUTH_TOKEN}"])
            .with_environment("CLAUDE_CODE_OAUTH_TOKEN", "${CLAUDE_CODE_OAUTH_TOKEN}")
            .with_environment("KEY", "${ANTHROPIC_API_KEY:-unset}")
            .with_environment("FOUNDRY", "${ANTHROPIC_FOUNDRY_API_KEY}")
            .with_environment("REFRESH", "${CLAUDE_CODE_OAUTH_REFRESH_TOKEN}")
            .with_environment("LOWER", "${claude_code_oauth_token}");
        let inherited = CliSettings::default().with_mcp_server("notes", server.clone());
        let explicit = CliSettings::default()
            .with_credential(Credential::key("ANTHROPIC_FOUNDRY_API_KEY", FOUNDRY))
            .with_mcp_server("notes", server);

        for (settings, foundry) in [(inherited, "${ANTHROPIC_FOUNDRY_API_KEY}"), (explicit, "")] {
            let attachment = attached(&settings);
            let environment =
                Environment::new(AgentKind::Claude, &settings, Some(&attachment), &host);
            let variables = set(&environment);

            for variable in attachment
                .environment
                .keys()
                .chain(attachment.templates.keys())
            {
                let value = variables[variable].clone().unwrap_or_default();
                for credential in [oauth, key, FOUNDRY, REFRESH, LOWER] {
                    assert!(
                        !value.contains(credential),
                        "{variable} holds a credential: {value}"
                    );
                }
            }
            let notes = &document(&attachment)["mcpServers"]["notes"];
            assert_eq!(expanded(&notes["args"][0], &variables), "--token=");
            for (variable, resolved) in [
                ("CLAUDE_CODE_OAUTH_TOKEN", ""),
                ("KEY", ""),
                ("FOUNDRY", foundry),
                ("REFRESH", ""),
                ("LOWER", ""),
            ] {
                assert_eq!(
                    expanded(&notes["env"][variable], &variables),
                    resolved,
                    "{variable}"
                );
            }
        }
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

    /// The marker tells a copy of the agent started from one of its own
    /// commands that it is nested, which it refuses or runs differently, so
    /// an allowed name never carries it; only an explicit value does.
    #[test]
    fn allowing_the_nested_session_marker_never_passes_it() {
        let settings = CliSettings::default().allow(["CLAUDECODE"]);
        let environment = Environment::new(AgentKind::Claude, &settings, None, &host());
        assert!(!set(&environment).contains_key("CLAUDECODE"));

        let explicit = settings.with_environment("CLAUDECODE", "1");
        let environment = Environment::new(AgentKind::Claude, &explicit, None, &host());
        assert_eq!(set(&environment)["CLAUDECODE"].as_deref(), Some("1"));
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

    /// A header can resolve to a lone separator, and scrubbing it would break
    /// every JSON line the run writes down.
    #[test]
    fn a_value_with_no_letter_or_digit_is_never_scrubbed() {
        let settings = CliSettings::default()
            .with_environment("SEPARATOR", "-")
            .with_mcp_server(
                "remote",
                McpServer::remote("https://mcp.example.com/mcp")
                    .with_header("Authorization", "${U:-}:${P:-}"),
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
