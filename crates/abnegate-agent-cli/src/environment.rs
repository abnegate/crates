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
use crate::mcp::Template;
use crate::mcp::expand;
use crate::mcp::references;
use crate::mcp::whole_reference;
use crate::settings::CLOUD_CREDENTIALS;
use crate::settings::CliSettings;

/// The proxy bypass list, which names hosts rather than holding a
/// credential, and whose hosts scrubbing would redact wherever a log
/// mentions them.
const BYPASS: &[&str] = &["NO_PROXY", "no_proxy"];

/// Claude Code's own OAuth refresh token, which it reads as empty in a stdio
/// server's values, as it does the OAuth token the agent signs in with, but
/// which the agent is never given.
const REFRESH_TOKEN: &str = "CLAUDE_CODE_OAUTH_REFRESH_TOKEN";

/// The locale categories POSIX and glibc define, which name a locale and
/// nothing secret. Named one by one rather than by their `LC_` prefix, since
/// any variable can be given that prefix to ride along wherever locales are
/// passed, as SSH's `AcceptEnv LC_*` passes them.
pub(crate) const LOCALE: &[&str] = &[
    "LC_ALL",
    "LC_CTYPE",
    "LC_COLLATE",
    "LC_MESSAGES",
    "LC_MONETARY",
    "LC_NUMERIC",
    "LC_TIME",
    "LC_ADDRESS",
    "LC_IDENTIFICATION",
    "LC_MEASUREMENT",
    "LC_NAME",
    "LC_PAPER",
    "LC_TELEPHONE",
];

/// Host variables an agent needs to run that hold nothing secret, given to
/// it beside [`DEFAULT_ENVIRONMENT`]: the user's shell, which the agent runs
/// its commands in, the per-user runtime directory, and where the
/// certificate authorities it verifies its API against are kept, for
/// OpenSSL and for Node.
pub(crate) const INHERITED: &[&str] = &[
    "SHELL",
    "XDG_RUNTIME_DIR",
    "SSL_CERT_DIR",
    "NODE_EXTRA_CA_CERTS",
];

/// What an [isolated](CliSettings::isolated) Claude run is given unless the
/// caller sets otherwise: no update of itself, no traffic it can do
/// without, and no memory carried from one run to the next.
const ISOLATION_DEFAULTS: &[(&str, &str)] = &[
    ("DISABLE_AUTOUPDATER", "1"),
    ("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1"),
    ("CLAUDE_CODE_DISABLE_AUTO_MEMORY", "1"),
];

/// Claude Code's waits on a call to an MCP tool, in milliseconds: the whole
/// call, and the time it may go without hearing from the server.
const TOOL_TIMEOUTS: &[&str] = &["MCP_TOOL_TIMEOUT", "CLAUDE_CODE_MCP_TOOL_IDLE_TIMEOUT"];

/// The child's environment: an allowlist of host variables, the
/// [locale](LOCALE) categories, the caller's allowed names and the agent's own
/// configuration variables, or the whole host environment when the caller opts
/// in, with every explicit value set on top. An allowed name the agent signs in
/// with passes only when its credential is inherited: beside a credential of
/// the caller's own, the host's would be one more the agent could sign in with.
/// With credentials refused, no other agent's sign-in name passes however it is
/// allowed or inherited, nor the agent's own but when it is signed in on the
/// host. With the host's credentials withheld, no name any agent signs in with,
/// and no [cloud sign-in](CLOUD_CREDENTIALS), passes from the host however it
/// is allowed or inherited, and no agent's configuration variable passes unless
/// it is allowed. Each allowed value but the proxy bypass list is a secret,
/// since a proxy URL, say, can carry a password.
///
/// Explicit values go on in rising precedence: the agent's sign-in variables
/// from the host when its credential is inherited, the [defaults] a Claude run
/// is given for its isolation and its MCP tools' timeout, the caller's public
/// variables and then its secret ones, the credential, and last the values an
/// MCP configuration moved out of its file, under generated names no caller can
/// know. A stdio server's references are resolved against the secrets bound to
/// it first, and then what the child is given before those, falling back to the
/// host, with the agent's sign-in variables and the credential's own read as
/// set but empty, and only the resolved values are handed over: resolving one
/// hands the child nothing under the name of the variable it refers to.
///
/// `Debug` names the variables and never prints a value: an inherited one,
/// a proxy URL say, can carry a password.
pub(crate) struct Environment {
    inherit: bool,
    removed: Vec<&'static str>,
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
            removed: removed(agent, settings),
            inherited: BTreeMap::new(),
            variables: BTreeMap::new(),
            secrets: Vec::new(),
        };
        let signed_in_on_host = matches!(settings.credential, Credential::Inherited)
            && !settings.withhold_host_credentials;
        if !environment.inherit {
            let configuration = if settings.withhold_host_credentials {
                &[]
            } else {
                agent.configuration()
            };
            for variable in DEFAULT_ENVIRONMENT
                .iter()
                .chain(INHERITED)
                .chain(LOCALE)
                .chain(configuration)
            {
                if let Some(value) = host(variable) {
                    environment.inherited.insert(variable.to_string(), value);
                }
            }
            for variable in &settings.allowed {
                let name = variable.as_str();
                let refused = environment.removed.contains(&name)
                    || (!signed_in_on_host && agent.credentials().contains(&name));
                if !refused {
                    environment.allow(variable, host);
                }
            }
        }
        if signed_in_on_host {
            for variable in agent.credentials() {
                environment.pass(variable, host);
            }
        }
        for (variable, value) in defaults(agent, settings, mcp.is_some())
            .into_iter()
            .chain(settings.variables.clone())
        {
            environment
                .variables
                .insert(variable, SecretValue::new(value));
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
    /// as it is, and each template resolved against the secrets bound to its
    /// server, and then what the child is given so far, falling back to the
    /// host, with every variable `agent` signs in with and the credential's
    /// own read as set but empty. Each is a secret, and so is every secret
    /// bound to a server in the file and every value a template's references
    /// resolved to, unless all it holds is public: the value of an
    /// allowlisted name or of one of the caller's public variables, bound to
    /// no secret.
    fn attach(
        &mut self,
        agent: AgentKind,
        mcp: &McpAttachment,
        settings: &CliSettings,
        host: &dyn Fn(&str) -> Option<OsString>,
    ) {
        let defaulted: Vec<String> = defaults(agent, settings, true)
            .into_iter()
            .map(|(variable, _)| variable)
            .collect();
        let public = |name: &str| {
            harmless(name)
                || settings.variables.contains_key(name)
                || defaulted.iter().any(|variable| variable == name)
        };
        let lookup = |name: &str| {
            if withheld(agent, settings, name) {
                return Some(String::new());
            }
            if let Some(value) = self.variables.get(name) {
                return Some(value.expose().to_string());
            }
            if unlent_configuration(settings, name) {
                return Some(String::new());
            }
            self.lookup(name, host)
        };
        let resolved: Vec<(&String, &Template, SecretValue)> = mcp
            .templates
            .iter()
            .map(|(variable, template)| {
                let read = |name: &str| template.bound(name).or_else(|| lookup(name));
                let value = expand(template.value.expose(), &read);
                (variable, template, SecretValue::new(value))
            })
            .collect();
        let referenced: Vec<SecretValue> = mcp
            .templates
            .values()
            .flat_map(|template| {
                references(template.value.expose())
                    .filter(move |name| !template.secrets.contains_key(*name))
            })
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
            let text = template.value.expose();
            let unbound_public = |name: &str| public(name) && !template.secrets.contains_key(name);
            if whole_reference(text) && references(text).all(unbound_public) {
                self.variables.insert(variable.clone(), value);
            } else {
                self.secret(template.value.clone());
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
            for variable in &self.removed {
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

    /// What the child is given for `variable`, with `host` reading this
    /// process's variables as it did when the environment was built: the
    /// host's value counts only when the child inherits the host's whole
    /// environment and the name is not removed from it.
    pub(crate) fn variable(
        &self,
        variable: &str,
        host: &dyn Fn(&str) -> Option<OsString>,
    ) -> Option<OsString> {
        if let Some(value) = self.variables.get(variable) {
            return Some(OsString::from(value.expose()));
        }
        if let Some(value) = self.inherited.get(variable) {
            return Some(value.clone());
        }
        if self.inherit && !self.removed.contains(&variable) {
            return host(variable);
        }
        None
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
    /// a secret unless it is [harmless](harmless) or is the proxy bypass list.
    fn allow(&mut self, variable: &str, host: &dyn Fn(&str) -> Option<OsString>) {
        let Some(value) = host(variable) else {
            return;
        };
        if !harmless(variable) && !BYPASS.contains(&variable) {
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

/// What a Claude run is given before the caller's own variables, which
/// override it, none of it secret: the [isolation defaults](ISOLATION_DEFAULTS)
/// when the run is [isolated](CliSettings::isolated), and, when `attached`,
/// the longest [tool timeout](crate::McpServer::tool_timeout) of the servers
/// attached to it as both of Claude Code's [waits](TOOL_TIMEOUTS) on a tool.
/// Codex is given neither.
fn defaults(agent: AgentKind, settings: &CliSettings, attached: bool) -> Vec<(String, String)> {
    if agent != AgentKind::Claude {
        return Vec::new();
    }
    let mut defaults: Vec<(String, String)> = Vec::new();
    if settings.isolated {
        defaults.extend(
            ISOLATION_DEFAULTS
                .iter()
                .map(|(variable, value)| ((*variable).to_string(), (*value).to_string())),
        );
    }
    let timeout = settings
        .mcp
        .attachable(agent)
        .filter(|_| attached)
        .filter_map(|(_, server)| server.tool_timeout)
        .max();
    if let Some(timeout) = timeout {
        let milliseconds = timeout.as_millis().to_string();
        defaults.extend(
            TOOL_TIMEOUTS
                .iter()
                .map(|variable| ((*variable).to_string(), milliseconds.clone())),
        );
    }
    defaults
}

/// Whether `variable` is one the child is given from the host by default,
/// none of which holds anything secret.
fn harmless(variable: &str) -> bool {
    DEFAULT_ENVIRONMENT.contains(&variable)
        || INHERITED.contains(&variable)
        || LOCALE.contains(&variable)
}

/// The host variables the child never inherits, and never passes when
/// allowed: the agent's nested-session marker; with credentials
/// [refused](CliSettings::refuse_credentials), every variable any agent
/// signs in with; with the host's credentials withheld, every
/// [sign-in](host_sign_in) the host could lend it, and every agent's
/// configuration variable the caller has not allowed when the child
/// inherits the host's whole environment.
fn removed(agent: AgentKind, settings: &CliSettings) -> Vec<&'static str> {
    let mut removed = agent.scrubbed().to_vec();
    if settings.refuse_credentials {
        removed.extend(credentials());
    }
    if settings.withhold_host_credentials {
        removed.extend(host_sign_in());
        if settings.inherit_environment {
            removed.extend(
                AgentKind::ALL
                    .iter()
                    .flat_map(|agent| agent.configuration())
                    .filter(|name| !settings.allowed.contains(**name)),
            );
        }
    }
    removed
}

/// Every variable any agent [signs in with](AgentKind::credentials).
fn credentials() -> impl Iterator<Item = &'static str> {
    AgentKind::ALL
        .iter()
        .flat_map(|agent| agent.credentials())
        .copied()
}

/// Every variable through which the host could lend an agent its own
/// sign-in: those any agent signs in with, Claude Code's OAuth refresh token
/// and the [cloud sign-ins](CLOUD_CREDENTIALS).
fn host_sign_in() -> impl Iterator<Item = &'static str> {
    credentials()
        .chain([REFRESH_TOKEN])
        .chain(CLOUD_CREDENTIALS.iter().copied())
}

/// Whether a stdio server's reference to `variable` reads it as set but
/// empty: a variable `agent` signs in with, or with credentials
/// [refused](CliSettings::refuse_credentials), one any agent signs in with,
/// or with the host's credentials withheld, every [sign-in](host_sign_in)
/// the host could lend it, the one
/// the credential occupies, or Claude Code's OAuth refresh token, in any
/// case. Claude Code reads its own OAuth tokens that way in a stdio server's
/// values, and a value resolved here reaches the child under a generated
/// name that every stdio server and the agent's own tools can read.
fn withheld(agent: AgentKind, settings: &CliSettings, variable: &str) -> bool {
    let matches = |name: &str| name.eq_ignore_ascii_case(variable);
    if settings.withhold_host_credentials && host_sign_in().any(matches) {
        return true;
    }
    if settings.refuse_credentials && credentials().any(matches) {
        return true;
    }
    agent
        .credentials()
        .iter()
        .copied()
        .chain(settings.credential.variable())
        .chain([REFRESH_TOKEN])
        .any(matches)
}

/// Whether a stdio server's reference to `variable`, which the caller has
/// not set, reads it as set but empty rather than from the host: with the
/// host's credentials withheld, an agent's configuration variable the caller
/// has not allowed, in any case.
fn unlent_configuration(settings: &CliSettings, variable: &str) -> bool {
    settings.withhold_host_credentials
        && AgentKind::ALL
            .iter()
            .flat_map(|agent| agent.configuration())
            .any(|name| name.eq_ignore_ascii_case(variable) && !settings.allowed.contains(*name))
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
    use std::time::Duration;

    use abnegate_llm::Credential;
    use abnegate_secret::SecretValue;
    use serde_json::Map;
    use serde_json::Value;
    use tokio::process::Command;

    use super::Environment;
    use super::REFRESH_TOKEN;
    use crate::kind::AgentKind;
    use crate::mcp::McpAttachment;
    use crate::mcp::McpConfig;
    use crate::mcp::McpServer;
    use crate::mcp::expand;
    use crate::scrubber::Scrubber;
    use crate::settings::CLOUD_CREDENTIALS;
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
        let file =
            std::fs::read_to_string(attachment.file.as_ref().expect("a rendered file").path())
                .expect("the file");
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

    /// Codex signs in with an OpenAI key, a Codex key or a Codex access
    /// token, and a stdio server's reference to any of them reads as set but
    /// empty, as Claude's own sign-in does, or the server Codex starts would
    /// be handed the agent's credential.
    #[test]
    fn a_codex_stdio_reference_to_codexs_sign_in_resolves_as_set_but_empty() {
        const CREDENTIALS: [(&str, &str); 3] = [
            ("OPENAI_API_KEY", "openai-host-marker"),
            ("CODEX_API_KEY", "codex-host-marker"),
            ("CODEX_ACCESS_TOKEN", "codex-access-marker"),
        ];
        let shared = host();
        let host = |name: &str| {
            CREDENTIALS
                .iter()
                .find(|(variable, _)| *variable == name)
                .map(|(_, value)| OsString::from(value))
                .or_else(|| shared(name))
        };
        let server = CREDENTIALS.iter().fold(
            McpServer::command("notes-server", ["--key=${CODEX_API_KEY}"]),
            |server, (variable, _)| server.with_environment(*variable, format!("${{{variable}}}")),
        );
        let settings = CliSettings::default().with_mcp_server("notes", server);
        let attachment = settings
            .mcp
            .render(AgentKind::Codex)
            .expect("rendered")
            .expect("an attachment");

        let environment = Environment::new(AgentKind::Codex, &settings, Some(&attachment), &host);

        let variables = set(&environment);
        assert!(!attachment.templates.is_empty());
        for variable in attachment
            .environment
            .keys()
            .chain(attachment.templates.keys())
        {
            let value = variables[variable].clone().unwrap_or_default();
            assert!(
                value.is_empty() || value == "--key=",
                "{variable} holds a credential: {value}"
            );
        }
    }

    /// A secret the caller binds to a stdio server is what its reference
    /// reads, even under a name the agent signs in with or one on the
    /// allowlist; it is scrubbed, and the names nothing binds are read as
    /// before.
    #[test]
    fn a_secret_bound_to_a_stdio_server_is_read_first_and_scrubbed() {
        let settings = CliSettings::default().with_mcp_server(
            "notes",
            McpServer::command("notes-server", ["--path=${PATH}"])
                .with_environment("OAUTH", "${CLAUDE_CODE_OAUTH_TOKEN}")
                .with_environment("KEY", "${ANTHROPIC_API_KEY}")
                .with_environment("GRAFANA", "${GRAFANA_TOKEN}")
                .with_secret("CLAUDE_CODE_OAUTH_TOKEN", "bound-oauth-marker")
                .with_secret("PATH", "bound-path-marker"),
        );
        let attachment = attached(&settings);
        let environment =
            Environment::new(AgentKind::Claude, &settings, Some(&attachment), &host());
        let variables = set(&environment);

        let notes = &document(&attachment)["mcpServers"]["notes"];
        assert_eq!(
            expanded(&notes["args"][0], &variables),
            "--path=bound-path-marker"
        );
        assert_eq!(
            expanded(&notes["env"]["OAUTH"], &variables),
            "bound-oauth-marker"
        );
        assert_eq!(expanded(&notes["env"]["KEY"], &variables), "");
        assert_eq!(
            expanded(&notes["env"]["GRAFANA"], &variables),
            "glsa-host-token"
        );
        let holders: Vec<&String> = variables
            .iter()
            .filter(|(_, value)| {
                value
                    .as_deref()
                    .is_some_and(|value| value.contains("bound-"))
            })
            .map(|(variable, _)| variable)
            .collect();
        assert!(
            holders
                .iter()
                .all(|variable| variable.starts_with("ABNEGATE_MCP_")),
            "a bound secret reached the child under a name of its own: {holders:?}"
        );
        let scrubber = Scrubber::new(environment.secrets());
        for secret in ["bound-oauth-marker", "bound-path-marker"] {
            assert_eq!(scrubber.scrub(secret), "[REDACTED]", "{secret}");
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

    /// A caller that allows a key the agent signs in with, for the agent's
    /// own scripts say, and signs the agent in with a credential of its own
    /// would otherwise hand the agent the host's key beside it, which the
    /// agent may sign in with instead. With the credential inherited, the
    /// host's keys are what the agent signs in with, so they still pass.
    #[test]
    fn a_passthrough_naming_a_credential_still_drops_it() {
        const HOST: &str = "notreal-host-credential";
        for (agent, credentials, variable, key) in [
            (
                AgentKind::Claude,
                &[
                    "ANTHROPIC_API_KEY",
                    "ANTHROPIC_AUTH_TOKEN",
                    "CLAUDE_CODE_OAUTH_TOKEN",
                ][..],
                "ANTHROPIC_API_KEY",
                concat!("sk-ant-", "explicit"),
            ),
            (
                AgentKind::Codex,
                &["OPENAI_API_KEY", "CODEX_API_KEY", "CODEX_ACCESS_TOKEN"][..],
                "OPENAI_API_KEY",
                concat!("sk-", "explicit"),
            ),
        ] {
            let host = |name: &str| credentials.contains(&name).then(|| OsString::from(HOST));
            let allowed = CliSettings::default().allow(credentials.iter().copied());
            let explicit = allowed
                .clone()
                .with_credential(Credential::key(variable, key));

            let variables = set(&Environment::new(agent, &explicit, None, &host));
            let carrying: Vec<&String> = variables
                .iter()
                .filter(|(_, value)| value.as_deref() == Some(HOST))
                .map(|(name, _)| name)
                .collect();
            assert!(
                carrying.is_empty(),
                "{agent} was handed the host's {carrying:?}"
            );
            assert_eq!(variables[variable].as_deref(), Some(key), "{agent}");

            let variables = set(&Environment::new(agent, &allowed, None, &host));
            for name in credentials {
                assert_eq!(
                    variables.get(*name).cloned().flatten().as_deref(),
                    Some(HOST),
                    "{agent} was not handed the host's {name} it signs in with"
                );
            }

            let withheld = allowed.clone().withhold_host_credentials();
            let variables = set(&Environment::new(agent, &withheld, None, &host));
            let carrying: Vec<&String> = variables
                .iter()
                .filter(|(_, value)| value.as_deref() == Some(HOST))
                .map(|(name, _)| name)
                .collect();
            assert!(
                carrying.is_empty(),
                "{agent} was handed the host's {carrying:?} though they were withheld"
            );
        }
    }

    /// Every variable an agent signs in with, its own or another agent's, or
    /// a cloud provider signs it in with, and Claude Code's refresh token, is
    /// refused from the host once the host's credentials are withheld, so an
    /// agent's credential cannot reach it by naming it, by inheriting the
    /// host's whole environment, or by being inherited. Only what the caller
    /// hands the run itself still does.
    #[test]
    fn every_variable_an_agent_signs_in_with_is_refused() {
        const HOST: &str = "notreal-host-credential";
        let credentials: Vec<&str> = AgentKind::ALL
            .iter()
            .flat_map(|agent| agent.credentials())
            .chain(CLOUD_CREDENTIALS)
            .chain([REFRESH_TOKEN].iter())
            .copied()
            .collect();
        let host = |name: &str| credentials.contains(&name).then(|| OsString::from(HOST));

        for agent in AgentKind::ALL {
            for inherit in [false, true] {
                let mut settings = CliSettings::default()
                    .allow(credentials.iter().copied())
                    .withhold_host_credentials();
                if inherit {
                    settings = settings.inherit_environment();
                }

                let environment = Environment::new(*agent, &settings, None, &host);
                let variables = set(&environment);
                for name in &credentials {
                    let value = variables.get(*name);
                    assert!(
                        if inherit {
                            value == Some(&None)
                        } else {
                            value.is_none()
                        },
                        "{agent}, inheriting {inherit}, can be handed the host's {name}: {value:?}"
                    );
                }
                assert!(exposed(&environment).is_empty(), "{agent}");

                let explicit = settings.with_credential(Credential::key(
                    agent.variable(),
                    concat!("sk-", "explicit"),
                ));
                let variables = set(&Environment::new(*agent, &explicit, None, &host));
                assert_eq!(
                    variables[agent.variable()].as_deref(),
                    Some(concat!("sk-", "explicit")),
                    "{agent}"
                );
            }
        }
    }

    /// The configuration variables point an agent at the host's stored
    /// sign-in, so with the host's credentials withheld they pass only when
    /// the caller names them, which is how a host points the agent at a
    /// sign-in kept somewhere else.
    #[test]
    fn a_withheld_hosts_configuration_passes_only_when_allowed() {
        let withheld = CliSettings::default().withhold_host_credentials();
        let variables = set(&Environment::new(
            AgentKind::Claude,
            &withheld,
            None,
            &host(),
        ));
        assert!(
            !variables.contains_key("CLAUDE_CONFIG_DIR"),
            "{variables:?}"
        );
        assert!(
            !variables.contains_key("ANTHROPIC_API_KEY"),
            "{variables:?}"
        );
        assert_eq!(variables["HOME"].as_deref(), Some("/home/agent"));

        let allowed = withheld.clone().allow(["CLAUDE_CONFIG_DIR"]);
        let variables = set(&Environment::new(
            AgentKind::Claude,
            &allowed,
            None,
            &host(),
        ));
        assert_eq!(
            variables["CLAUDE_CONFIG_DIR"].as_deref(),
            Some("/home/agent/.claude-work")
        );

        let inheriting = withheld.clone().inherit_environment();
        let variables = set(&Environment::new(
            AgentKind::Claude,
            &inheriting,
            None,
            &host(),
        ));
        assert_eq!(variables.get("CLAUDE_CONFIG_DIR"), Some(&None));
        assert_eq!(variables.get("CODEX_HOME"), Some(&None));

        let explicit = withheld.with_variable("CLAUDE_CONFIG_DIR", "/state/tenant/claude");
        let variables = set(&Environment::new(
            AgentKind::Claude,
            &explicit,
            None,
            &host(),
        ));
        assert_eq!(
            variables["CLAUDE_CONFIG_DIR"].as_deref(),
            Some("/state/tenant/claude")
        );
    }

    /// A stdio server's reference to a variable any agent signs in with
    /// reads as set but empty once the host's credentials are withheld, or
    /// the generated variable it resolves to would hand the agent the host's.
    #[test]
    fn a_stdio_reference_to_any_agents_sign_in_reads_as_empty_when_withheld() {
        const OPENAI: &str = "notreal-openai-host-key";
        let shared = host();
        let host = |name: &str| match name {
            "OPENAI_API_KEY" => Some(OsString::from(OPENAI)),
            _ => shared(name),
        };
        let server = McpServer::command("notes-server", Vec::<String>::new())
            .with_environment("OPENAI", "${OPENAI_API_KEY}");
        let inherited = CliSettings::default().with_mcp_server("notes", server);
        let withheld = inherited.clone().withhold_host_credentials();

        for (settings, resolved) in [(inherited, OPENAI), (withheld, "")] {
            let attachment = attached(&settings);
            let environment =
                Environment::new(AgentKind::Claude, &settings, Some(&attachment), &host);
            let variables = set(&environment);
            let notes = &document(&attachment)["mcpServers"]["notes"];
            assert_eq!(expanded(&notes["env"]["OPENAI"], &variables), resolved);
        }
    }

    /// Without the host's credentials withheld, an agent signed in on the
    /// host is still handed its own sign-in, but no other agent's, however
    /// it is allowed or inherited, and a stdio server's reference to another
    /// agent's reads as empty.
    #[test]
    fn refused_credentials_never_pass_from_the_host_but_the_agents_own_sign_in() {
        const OPENAI: &str = "notreal-openai-host-key";
        let shared = host();
        let host = |name: &str| match name {
            "OPENAI_API_KEY" => Some(OsString::from(OPENAI)),
            _ => shared(name),
        };
        let allowed = CliSettings::default().allow(["OPENAI_API_KEY", "LINEAR_API_URL"]);

        let variables = set(&Environment::new(AgentKind::Claude, &allowed, None, &host));
        assert_eq!(
            variables["OPENAI_API_KEY"].as_deref(),
            Some(OPENAI),
            "another agent's key passes when allowed and not refused"
        );

        for inherit in [false, true] {
            let mut refused = allowed.clone().refuse_credentials();
            if inherit {
                refused = refused.inherit_environment();
            }
            let variables = set(&Environment::new(AgentKind::Claude, &refused, None, &host));
            let passed = variables.get("OPENAI_API_KEY");
            assert!(
                if inherit {
                    passed == Some(&None)
                } else {
                    passed.is_none()
                },
                "inheriting {inherit}, codex's key reached claude: {passed:?}"
            );
            assert_eq!(
                variables["ANTHROPIC_API_KEY"].as_deref(),
                Some(concat!("sk-ant-", "host-key")),
                "inheriting {inherit}, claude lost the sign-in it runs on"
            );
            assert_eq!(
                variables["CLAUDE_CODE_OAUTH_TOKEN"].as_deref(),
                Some(concat!("sk-ant-", "oat-host-token")),
                "inheriting {inherit}"
            );
            if !inherit {
                assert_eq!(
                    variables["LINEAR_API_URL"].as_deref(),
                    Some("https://linear.internal")
                );
            }
        }

        let server = McpServer::command("notes-server", Vec::<String>::new())
            .with_environment("OPENAI", "${OPENAI_API_KEY}");
        let settings = allowed
            .refuse_credentials()
            .with_mcp_server("notes", server);
        let attachment = attached(&settings);
        let environment = Environment::new(AgentKind::Claude, &settings, Some(&attachment), &host);
        let variables = set(&environment);
        let notes = &document(&attachment)["mcpServers"]["notes"];
        assert_eq!(expanded(&notes["env"]["OPENAI"], &variables), "");
    }

    /// What a host lends every agent beside the defaults: the shell it runs
    /// commands in, the runtime directory, and the certificate authorities
    /// its API is verified against. None of it is a secret, allowed by name
    /// or not.
    #[test]
    fn what_an_agent_needs_to_run_is_inherited_without_being_allowed() {
        let host = |name: &str| {
            let value = match name {
                "SHELL" => "/bin/zsh",
                "XDG_RUNTIME_DIR" => "/run/user/1000",
                "SSL_CERT_DIR" => "/etc/ssl/certs",
                "NODE_EXTRA_CA_CERTS" => "/etc/ssl/certs/extra.pem",
                "LC_CTYPE" => "UTF-8",
                _ => return None,
            };
            Some(OsString::from(value))
        };

        for settings in [
            CliSettings::default(),
            CliSettings::default().allow(["SHELL", "SSL_CERT_DIR", "LC_CTYPE"]),
        ] {
            for agent in AgentKind::ALL {
                let environment = Environment::new(*agent, &settings, None, &host);
                let variables = set(&environment);
                for (name, value) in [
                    ("SHELL", "/bin/zsh"),
                    ("XDG_RUNTIME_DIR", "/run/user/1000"),
                    ("SSL_CERT_DIR", "/etc/ssl/certs"),
                    ("NODE_EXTRA_CA_CERTS", "/etc/ssl/certs/extra.pem"),
                ] {
                    assert_eq!(
                        variables.get(name).cloned().flatten().as_deref(),
                        Some(value),
                        "{agent} was not handed {name}"
                    );
                }
                assert!(
                    exposed(&environment).is_empty(),
                    "{agent}: {:?}",
                    exposed(&environment)
                );
            }
        }
    }

    /// The recipe a host running agents for several organisations uses,
    /// against what such a host's own environment holds: its database, its
    /// signing keys, its gateway's keys, the agents' own sign-ins and stored
    /// state, the nested-session markers and the token it hands each run.
    fn hosted() -> CliSettings {
        CliSettings::default()
            .withhold_host_credentials()
            .refuse_credentials()
            .with_proxy_variables()
            .allow(["ANTHROPIC_API_KEY", "OPENAI_API_KEY"])
    }

    #[test]
    fn a_hosts_secrets_and_the_agents_own_state_are_never_inherited() {
        let refused: BTreeMap<&str, &str> = [
            ("DATABASE_URL", "postgres://app:notrealpassword@db/app"),
            ("JWT_SECRET", "jwt-notarealsecret"),
            ("ENCRYPTION_KEY", "encryption-notarealkey"),
            ("LITELLM_KEY", "sk-litellm-notarealkey"),
            ("SECURITY_LITELLM_MASTER_KEY", "sk-litellm-notarealkey"),
            ("ANTHROPIC_API_KEY", "sk-ant-notarealkey"),
            ("OPENAI_API_KEY", "sk-notarealkey"),
            ("CLAUDE_CONFIG_DIR", "/home/host/.claude-server"),
            ("CODEX_HOME", "/home/host/.codex-server"),
            ("CLAUDECODE", "1"),
            ("CLAUDE_CODE_ENTRYPOINT", "cli"),
            ("HOST_MCP_TOKEN", "host-turn-leftover"),
        ]
        .into();
        let host = |name: &str| match name {
            "PATH" => Some(OsString::from("/usr/bin")),
            _ => refused.get(name).map(OsString::from),
        };

        for agent in AgentKind::ALL {
            let environment = Environment::new(*agent, &hosted(), None, &host);
            let variables = set(&environment);
            for name in refused.keys() {
                assert!(
                    !variables.contains_key(*name),
                    "{name} reached {agent}: {variables:?}"
                );
            }
            assert_eq!(variables["PATH"].as_deref(), Some("/usr/bin"), "{agent}");
            assert!(exposed(&environment).is_empty(), "{agent}");
        }
    }

    #[test]
    fn what_an_agent_needs_to_run_reaches_it_unchanged_under_a_hosts_recipe() {
        let kept: BTreeMap<&str, &str> = [
            ("HOME", "/home/host"),
            ("PATH", "/usr/local/bin:/usr/bin:/bin"),
            ("LANG", "C.UTF-8"),
            ("LC_ALL", "C.UTF-8"),
            ("LC_CTYPE", "UTF-8"),
            ("HTTPS_PROXY", "http://proxy:3128"),
            ("SSL_CERT_FILE", "/etc/ssl/certs/ca-certificates.crt"),
            ("NODE_EXTRA_CA_CERTS", "/etc/ssl/certs/extra.pem"),
            ("XDG_RUNTIME_DIR", "/run/user/1000"),
        ]
        .into();
        let host = |name: &str| kept.get(name).map(OsString::from);

        for agent in AgentKind::ALL {
            let variables = set(&Environment::new(*agent, &hosted(), None, &host));
            for (name, value) in &kept {
                assert_eq!(
                    variables.get(*name).cloned().flatten().as_deref(),
                    Some(*value),
                    "{name} did not reach {agent} as it was"
                );
            }
        }
    }

    #[test]
    fn a_stdio_reference_to_an_agents_configuration_reads_as_empty_when_withheld_unless_allowed() {
        const CONFIGURATION: &str = "/home/host/.claude";
        let shared = host();
        let host = |name: &str| match name {
            "CLAUDE_CONFIG_DIR" => Some(OsString::from(CONFIGURATION)),
            _ => shared(name),
        };
        let server = McpServer::command("notes-server", Vec::<String>::new())
            .with_environment("CONFIGURATION", "${CLAUDE_CONFIG_DIR}");
        let withheld = CliSettings::default()
            .with_mcp_server("notes", server)
            .withhold_host_credentials();

        for (settings, resolved) in [
            (withheld.clone(), ""),
            (withheld.clone().inherit_environment(), ""),
            (withheld.clone().allow(["CLAUDE_CONFIG_DIR"]), CONFIGURATION),
            (
                withheld
                    .clone()
                    .with_environment("CLAUDE_CONFIG_DIR", "/tenant"),
                "/tenant",
            ),
            (
                withheld.with_variable("CLAUDE_CONFIG_DIR", "/tenant"),
                "/tenant",
            ),
        ] {
            let attachment = attached(&settings);
            let environment =
                Environment::new(AgentKind::Codex, &settings, Some(&attachment), &host);
            let variables = set(&environment);
            let notes = &document(&attachment)["mcpServers"]["notes"];
            assert_eq!(
                expanded(&notes["env"]["CONFIGURATION"], &variables),
                resolved,
                "{settings:?}"
            );
        }
    }

    #[test]
    fn an_azure_identity_is_withheld_with_the_other_cloud_sign_ins() {
        for name in [
            "AZURE_CLIENT_ID",
            "AZURE_TENANT_ID",
            "AZURE_CLIENT_SECRET",
            "AZURE_CLIENT_CERTIFICATE_PATH",
            "AZURE_CLIENT_CERTIFICATE_PASSWORD",
            "AZURE_USERNAME",
            "AZURE_PASSWORD",
            "AZURE_FEDERATED_TOKEN_FILE",
        ] {
            assert!(CLOUD_CREDENTIALS.contains(&name), "{name} is not withheld");
        }
    }

    /// Only the locale categories pass, each by name: a variable merely
    /// given their prefix, as a token can be to ride along with them, does
    /// not.
    #[test]
    fn locale_variables_pass_through_as_a_set() {
        let host = |name: &str| {
            let value = match name {
                "LC_ALL" => "C.UTF-8",
                "LC_CTYPE" => "UTF-8",
                "LC_TELEPHONE" => "en_GB.UTF-8",
                "LC_TIME" => "C",
                "LC_TERMINAL" => "iTerm2",
                "LC_SECRET_TOKEN" => "notreal-smuggled-token",
                "LCX" => "not-a-locale",
                "DATABASE_URL" => "postgres://app:notrealpassword@db/app",
                _ => return None,
            };
            Some(OsString::from(value))
        };
        let settings = CliSettings::default().with_variable("LC_TIME", "en_GB.UTF-8");
        let environment = Environment::new(AgentKind::Claude, &settings, None, &host);

        let variables = set(&environment);
        assert_eq!(variables["LC_ALL"].as_deref(), Some("C.UTF-8"));
        assert_eq!(variables["LC_CTYPE"].as_deref(), Some("UTF-8"));
        assert_eq!(variables["LC_TELEPHONE"].as_deref(), Some("en_GB.UTF-8"));
        assert_eq!(
            variables["LC_TIME"].as_deref(),
            Some("en_GB.UTF-8"),
            "the explicit value lost to the host's"
        );
        for name in ["LC_TERMINAL", "LC_SECRET_TOKEN", "LCX", "DATABASE_URL"] {
            assert!(
                !variables.contains_key(name),
                "{name} passed: {variables:?}"
            );
        }
        assert!(exposed(&environment).is_empty(), "a locale is no secret");

        let inheriting = Environment::new(
            AgentKind::Claude,
            &CliSettings::default().inherit_environment(),
            None,
            &host,
        );
        assert!(
            !set(&inheriting).contains_key("LC_CTYPE"),
            "inherited, not set"
        );
    }

    /// Only the names the agent signs in with are dropped: whatever else the
    /// caller allows beside them still passes.
    #[test]
    fn a_passthrough_naming_a_harmless_variable_still_passes_it() {
        let settings = CliSettings::default()
            .with_credential(Credential::key(
                "ANTHROPIC_API_KEY",
                concat!("sk-ant-", "explicit"),
            ))
            .allow([
                "LINEAR_API_URL",
                "CLAUDE_CONFIG_DIR",
                "GITHUB_TOKEN",
                "CLAUDE_CODE_OAUTH_TOKEN",
            ]);
        let environment = Environment::new(AgentKind::Claude, &settings, None, &host());

        let variables = set(&environment);
        assert_eq!(
            variables["LINEAR_API_URL"].as_deref(),
            Some("https://linear.internal")
        );
        assert_eq!(
            variables["CLAUDE_CONFIG_DIR"].as_deref(),
            Some("/home/agent/.claude-work")
        );
        assert_eq!(variables["GITHUB_TOKEN"].as_deref(), Some("ghp-host-token"));
        assert!(!variables.contains_key("CLAUDE_CODE_OAUTH_TOKEN"));
    }

    #[test]
    fn the_credential_outranks_a_variable_of_the_same_name() {
        const CREDENTIAL: &str = concat!("sk-ant-", "oat01-the-credential");
        let settings = CliSettings::default()
            .with_variable("CLAUDE_CODE_OAUTH_TOKEN", "set-as-a-variable")
            .with_credential(Credential::key("CLAUDE_CODE_OAUTH_TOKEN", CREDENTIAL));
        let environment = Environment::new(AgentKind::Claude, &settings, None, &host());

        assert_eq!(
            set(&environment)["CLAUDE_CODE_OAUTH_TOKEN"].as_deref(),
            Some(CREDENTIAL)
        );
    }

    #[test]
    fn the_variables_the_caller_sets_reach_the_agent_over_what_it_would_inherit() {
        let settings = CliSettings::default()
            .with_variable("CLAUDE_CONFIG_DIR", "/state/tenant/claude")
            .with_variable("DISABLE_AUTOUPDATER", "1")
            .with_variable("HOME", "/state/tenant/home");
        let environment = Environment::new(AgentKind::Claude, &settings, None, &host());

        let variables = set(&environment);
        for (name, value) in [
            ("CLAUDE_CONFIG_DIR", "/state/tenant/claude"),
            ("DISABLE_AUTOUPDATER", "1"),
            ("HOME", "/state/tenant/home"),
        ] {
            assert_eq!(variables[name].as_deref(), Some(value), "{name}");
        }
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
        let file =
            std::fs::read_to_string(attachment.file.as_ref().expect("a rendered file").path())
                .expect("the file");
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

    const ISOLATION: [(&str, &str); 3] = [
        ("DISABLE_AUTOUPDATER", "1"),
        ("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1"),
        ("CLAUDE_CODE_DISABLE_AUTO_MEMORY", "1"),
    ];

    const TIMEOUTS: [&str; 2] = ["MCP_TOOL_TIMEOUT", "CLAUDE_CODE_MCP_TOOL_IDLE_TIMEOUT"];

    /// An isolated Claude run neither updates itself, nor phones home, nor
    /// remembers across runs, unless the caller says otherwise, and none of
    /// it is a secret, so a log keeps every `1` it holds.
    #[test]
    fn an_isolated_claude_run_is_given_its_defaults_and_the_caller_overrides_them() {
        let environment = Environment::new(
            AgentKind::Claude,
            &CliSettings::default().isolate(),
            None,
            &host(),
        );
        let variables = set(&environment);
        for (name, value) in ISOLATION {
            assert_eq!(variables[name].as_deref(), Some(value), "{name}");
        }
        assert!(
            !exposed(&environment).iter().any(|secret| secret == "1"),
            "{:?}",
            exposed(&environment)
        );

        let settings = CliSettings::default()
            .isolate()
            .with_variable("DISABLE_AUTOUPDATER", "0")
            .with_environment("CLAUDE_CODE_DISABLE_AUTO_MEMORY", "0");
        let variables = set(&Environment::new(
            AgentKind::Claude,
            &settings,
            None,
            &host(),
        ));
        assert_eq!(variables["DISABLE_AUTOUPDATER"].as_deref(), Some("0"));
        assert_eq!(
            variables["CLAUDE_CODE_DISABLE_AUTO_MEMORY"].as_deref(),
            Some("0")
        );
        assert_eq!(
            variables["CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC"].as_deref(),
            Some("1")
        );

        for (agent, settings) in [
            (AgentKind::Claude, CliSettings::default()),
            (AgentKind::Codex, CliSettings::default().isolate()),
        ] {
            let variables = set(&Environment::new(agent, &settings, None, &host()));
            for (name, _) in ISOLATION {
                assert!(!variables.contains_key(name), "{agent}: {name}");
            }
        }
    }

    /// Claude Code has one wait on an MCP tool for every server, so a run is
    /// given the longest any attached server asks for, in milliseconds, in
    /// both of its variables, which the caller can still override.
    #[test]
    fn a_claude_run_waits_on_its_mcp_tools_as_long_as_its_longest_server_asks() {
        let settings = CliSettings::default()
            .with_mcp_server(
                "host",
                McpServer::remote("http://127.0.0.1:9/mcp")
                    .with_tool_timeout(Duration::from_secs(1800)),
            )
            .with_mcp_server(
                "relay",
                McpServer::remote("http://127.0.0.1:9/relay")
                    .with_tool_timeout(Duration::from_millis(90_500)),
            )
            .with_mcp_server(
                "slow",
                McpServer::remote("http://127.0.0.1:9/slow")
                    .with_tool_timeout(Duration::from_secs(3600))
                    .disable(),
            )
            .with_mcp_server("plain", McpServer::remote("http://127.0.0.1:9/plain"));
        let attachment = attached(&settings);
        let environment =
            Environment::new(AgentKind::Claude, &settings, Some(&attachment), &host());

        let variables = set(&environment);
        for name in TIMEOUTS {
            assert_eq!(variables[name].as_deref(), Some("1800000"), "{name}");
        }
        assert!(
            !exposed(&environment)
                .iter()
                .any(|secret| secret == "1800000"),
            "a timeout is no secret"
        );

        let overridden = settings.clone().with_variable("MCP_TOOL_TIMEOUT", "60000");
        let variables = set(&Environment::new(
            AgentKind::Claude,
            &overridden,
            Some(&attachment),
            &host(),
        ));
        assert_eq!(variables["MCP_TOOL_TIMEOUT"].as_deref(), Some("60000"));
        assert_eq!(
            variables["CLAUDE_CODE_MCP_TOOL_IDLE_TIMEOUT"].as_deref(),
            Some("1800000")
        );

        let unattached = set(&Environment::new(
            AgentKind::Claude,
            &settings,
            None,
            &host(),
        ));
        let codex = set(&Environment::new(
            AgentKind::Codex,
            &settings,
            None,
            &host(),
        ));
        let untimed = CliSettings::default()
            .with_mcp_server("plain", McpServer::remote("http://127.0.0.1:9/plain"));
        let untimed_attachment = attached(&untimed);
        let untimed = set(&Environment::new(
            AgentKind::Claude,
            &untimed,
            Some(&untimed_attachment),
            &host(),
        ));
        for variables in [unattached, codex, untimed] {
            for name in TIMEOUTS {
                assert!(!variables.contains_key(name), "{name}: {variables:?}");
            }
        }
    }

    /// A stdio server that refers to a default it is given reads it as
    /// public, so the `1` it holds is never scrubbed from every log line.
    #[test]
    fn a_stdio_reference_to_a_default_never_makes_it_a_secret() {
        let settings = CliSettings::default().isolate().with_mcp_server(
            "notes",
            McpServer::command("notes-server", ["mcp"])
                .with_environment("NOTES_QUIET", "${DISABLE_AUTOUPDATER}"),
        );
        let attachment = attached(&settings);
        let environment =
            Environment::new(AgentKind::Claude, &settings, Some(&attachment), &host());

        assert!(
            !exposed(&environment).iter().any(|secret| secret == "1"),
            "{:?}",
            exposed(&environment)
        );
    }
}
