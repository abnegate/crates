use std::time::Duration;

use crate::mcp::inline_table::InlineTable;
use crate::mcp::launch::Launch;
use crate::mcp::placeholders::Placeholders;
use crate::mcp::placeholders::refers;
use crate::mcp::refusal::Refusal;
use crate::mcp::server::McpServer;
use crate::mcp::transport::McpTransport;

const CONFIG: &str = "-c";
const SERVERS: &str = "mcp_servers";
const ENABLED_TOOLS: &str = "enabled_tools";
const TOOL_TIMEOUT: &str = "tool_timeout_sec";
const STARTUP_TIMEOUT: &str = "startup_timeout_sec";
const APPROVAL_MODE: &str = "default_tools_approval_mode";
const APPROVE: &str = "approve";
const REQUIRED: &str = "required";
const OMIT_TOOLS_FROM: &str = "omit_tools_from";
const DEFERRED: &str = "deferred";
const SHELL: &str = "/bin/sh";
const SHELL_SCRIPT: &str = "-c";
const SHELL_NAME: &str = "sh";

/// One server as a Codex run is given it: a `-c mcp_servers.<name>={...}`
/// override in Codex's own `config.toml` shape.
///
/// Codex never expands a reference, and anything in an override shows on its
/// command line, so no value that may be secret goes into one. It starts a
/// stdio server with a fixed handful of its own variables and those the
/// server's `env_vars` names, under those same names, so a stdio server that
/// is given variables is started through `/bin/sh`, whose script sets each
/// of them from the generated variable holding its value, drops the
/// generated one, and replaces itself with the server. The script names
/// variables alone, and every literal command and argument reaches it as a
/// positional parameter, never as script text. Only that server is handed
/// its generated variables. A remote server's header values reach Codex the
/// same way, as generated variables its `env_http_headers` names.
///
/// Every call to the server's tools is approved in advance, with
/// `default_tools_approval_mode = "approve"`, unless the server is
/// [supervised](McpServer::supervised): `codex exec` has nobody to ask, and a
/// call it would otherwise stop to ask about is decided by whoever serves the
/// tool. The server's [`tools`](McpServer::tools) become its
/// `enabled_tools`, always when it is
/// [exclusive](McpServer::exclusive), and its timeouts, whether it is
/// [required](McpServer::required) and whether its tools are listed
/// [eagerly](McpServer::eager) become keys of their own.
pub(crate) struct CodexServer<'server> {
    server: &'server McpServer,
}

impl<'server> CodexServer<'server> {
    pub(crate) fn new(server: &'server McpServer) -> Self {
        Self { server }
    }

    /// Why Codex could not be given this server without showing a value
    /// that may be secret, or reaching it at all, beyond what
    /// [`McpServer::refusal`] refuses for every CLI.
    pub(crate) fn refusal(&self) -> Option<Refusal> {
        if self.server.command.is_some() {
            let unnamed = self
                .server
                .environment
                .keys()
                .any(|variable| !identifier(variable));
            return unnamed.then_some(Refusal::VariableName);
        }
        if self.server.transport == Some(McpTransport::Sse) {
            return Some(Refusal::ServerSentEvents);
        }
        let url = self.server.url.as_deref()?;
        if refers(url) {
            return Some(Refusal::ReferringUrl);
        }
        revealing(url).then_some(Refusal::RevealingUrl)
    }

    /// The `-c` override that attaches this server under `name`, moving
    /// every value that may be secret into a generated variable in
    /// `placeholders`.
    ///
    /// Only for a server neither [`McpServer::refusal`] nor
    /// [`CodexServer::refusal`] refuses: the script sets each variable by
    /// its configured name, which only that check proves is an identifier.
    pub(crate) fn arguments(&self, name: &str, placeholders: &mut Placeholders) -> [String; 2] {
        let table = match self.launch(placeholders) {
            Some(launch) => match &self.server.working_directory {
                Some(directory) => launch
                    .table()
                    .with_text("cwd", &directory.to_string_lossy()),
                None => launch.table(),
            },
            None => self.remote(placeholders),
        };
        let table = self.settings(table);
        placeholders
            .secrets
            .extend(self.server.secrets.values().cloned());
        [CONFIG.to_string(), format!("{SERVERS}.{name}={table}")]
    }

    /// `table` with the server's tools and its settings for Codex: the tool
    /// list, the timeouts, the approval mode unless it is supervised, whether
    /// it is required, and where its tools are listed.
    fn settings(&self, table: InlineTable) -> InlineTable {
        let server = self.server;
        let mut table = table;
        if server.exclusive || !server.tools.is_empty() {
            table = table.with_texts(ENABLED_TOOLS, &server.tools);
        }
        if let Some(timeout) = server.tool_timeout {
            table = table.with_integer(TOOL_TIMEOUT, seconds(timeout));
        }
        if let Some(timeout) = server.startup_timeout {
            table = table.with_integer(STARTUP_TIMEOUT, seconds(timeout));
        }
        if !server.supervised {
            table = table.with_text(APPROVAL_MODE, APPROVE);
        }
        if server.required {
            table = table.with_boolean(REQUIRED, true);
        }
        if server.eager {
            table = table.with_texts(OMIT_TOOLS_FROM, [DEFERRED]);
        }
        table
    }

    /// How Codex starts this server, or none for a remote one: as configured
    /// when it is given no variable and refers to none, and otherwise
    /// through the shell.
    pub(crate) fn launch(&self, placeholders: &mut Placeholders) -> Option<Launch> {
        let command = self.server.command.as_ref()?;
        let secrets = &self.server.secrets;
        let mut literals: Vec<String> = Vec::new();
        let mut variables: Vec<String> = Vec::new();
        let mut words: Vec<String> = Vec::new();
        for text in std::iter::once(command).chain(&self.server.arguments) {
            match placeholders.template_for(text, secrets) {
                Some(variable) => {
                    words.push(format!("\"${variable}\""));
                    variables.push(variable);
                }
                None => {
                    literals.push(text.clone());
                    words.push(format!("\"${{{}}}\"", literals.len()));
                }
            }
        }
        let mut exports = String::new();
        for (variable, value) in &self.server.environment {
            match placeholders.variable_for(value, secrets) {
                Some(holder) => {
                    exports.push_str(&format!("export {variable}=\"${holder}\"; "));
                    variables.push(holder);
                }
                None => exports.push_str(&format!("export {variable}=; ")),
            }
        }

        if exports.is_empty() && variables.is_empty() {
            return Some(Launch {
                command: command.clone(),
                arguments: self.server.arguments.clone(),
                variables,
            });
        }
        let dropped = match variables.is_empty() {
            true => String::new(),
            false => format!("unset {}; ", variables.join(" ")),
        };
        let script = format!("{exports}set -- {}; {dropped}exec \"$@\"", words.join(" "));
        let arguments = [SHELL_SCRIPT.to_string(), script, SHELL_NAME.to_string()]
            .into_iter()
            .chain(literals)
            .collect();
        Some(Launch {
            command: SHELL.to_string(),
            arguments,
            variables,
        })
    }

    fn remote(&self, placeholders: &mut Placeholders) -> InlineTable {
        let url = self.server.url.as_deref().unwrap_or_default();
        let mut held = InlineTable::default();
        let mut empty = InlineTable::default();
        for (header, value) in &self.server.headers {
            match placeholders.variable_holding(self.server.sent(value.expose())) {
                Some(variable) => held = held.with_text(header, &variable),
                None => empty = empty.with_text(header, ""),
            }
        }
        let table = InlineTable::default().with_text("url", url);
        let table = match held.is_empty() {
            true => table,
            false => table.with_table("env_http_headers", held),
        };
        match empty.is_empty() {
            true => table,
            false => table.with_table("http_headers", empty),
        }
    }
}

/// `duration` in whole seconds, rounded up so that a wait shorter than a
/// second is never written as none at all.
fn seconds(duration: Duration) -> u64 {
    duration.as_secs() + u64::from(duration.subsec_nanos() > 0)
}

/// Whether `url` holds user-info or a query string, either of which can
/// carry a credential. Read as widely as any parser could: `\` ends the
/// authority as `/` does, and a `?` anywhere counts.
fn revealing(url: &str) -> bool {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let authority = rest.split(['/', '\\', '?', '#']).next().unwrap_or_default();
    authority.contains('@') || url.contains('?')
}

/// Whether `name` is a shell identifier: a letter or `_`, then letters,
/// digits and `_`.
fn identifier(name: &str) -> bool {
    let mut characters = name.chars();
    characters
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && characters.all(|character| character.is_ascii_alphanumeric() || character == '_')
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::process::Command;
    use std::time::Duration;

    use super::CodexServer;
    use crate::mcp::McpServer;
    use crate::mcp::McpTransport;
    use crate::mcp::expand;
    use crate::mcp::placeholders::NAMESPACE;
    use crate::mcp::placeholders::Placeholders;
    use crate::mcp::refusal::Refusal;

    const SECRET: &str = "glsa-literal-marker-4b2e";
    const BOUND: &str = "bound-marker-8d1f";

    /// What Codex hands the server a launch starts: each of its generated
    /// variables, resolved as the run's environment resolves it.
    fn handed(placeholders: &Placeholders, variables: &[String]) -> BTreeMap<String, String> {
        variables
            .iter()
            .map(|variable| {
                let value = match placeholders.environment.get(variable) {
                    Some(value) => value.expose().to_string(),
                    None => {
                        let template = &placeholders.templates[variable];
                        expand(template.value.expose(), &|name| template.bound(name))
                    }
                };
                (variable.clone(), value)
            })
            .collect()
    }

    /// The server the shell starts sees each of its variables under its own
    /// name, and none of the generated ones, and every literal argument
    /// arrives whole, however much shell syntax it holds.
    #[test]
    fn the_shell_hands_a_stdio_server_its_variables_under_their_own_names() {
        let hostile = "a b; exit 3 \"$(exit 4)\" `exit 5` $HOME";
        let server = McpServer::command(
            "/bin/sh",
            [
                "-c",
                "env; printf 'argument=%s\\n' \"$1\" \"$2\"",
                "server",
                hostile,
                "${ARGUMENT}",
            ],
        )
        .with_environment("TOKEN", SECRET)
        .with_environment("KEY", "${KEY}")
        .with_environment("EMPTY", "")
        .with_secret("KEY", BOUND)
        .with_secret("ARGUMENT", "argument-marker");
        let mut placeholders = Placeholders::under("TEST");

        let launch = CodexServer::new(&server)
            .launch(&mut placeholders)
            .expect("a stdio launch");
        let output = Command::new(&launch.command)
            .args(&launch.arguments)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .envs(handed(&placeholders, &launch.variables))
            .output()
            .expect("the shell runs");

        assert!(output.status.success(), "{output:?}");
        let printed = String::from_utf8(output.stdout).expect("text");
        assert!(printed.contains(&format!("TOKEN={SECRET}\n")), "{printed}");
        assert!(printed.contains(&format!("KEY={BOUND}\n")), "{printed}");
        assert!(printed.contains("EMPTY=\n"), "{printed}");
        assert!(!printed.contains(NAMESPACE), "{printed}");
        assert!(
            printed.contains(&format!("argument={hostile}\n")),
            "{printed}"
        );
        assert!(printed.contains("argument=argument-marker\n"), "{printed}");
    }

    /// Nothing that may be secret shows in the arguments, which reach
    /// Codex's command line: a literal value, a bound secret, or a default.
    #[test]
    fn no_value_that_may_be_secret_reaches_the_override() {
        let server = McpServer::command("uvx", ["mcp-server-appwrite"])
            .with_environment("TOKEN", SECRET)
            .with_environment("KEY", "${KEY:-fallback-marker}")
            .with_secret("KEY", BOUND);
        let mut placeholders = Placeholders::under("TEST");

        let arguments = CodexServer::new(&server).arguments("appwrite", &mut placeholders);

        assert_eq!(arguments[0], "-c");
        assert!(
            arguments[1].starts_with(r#"mcp_servers.appwrite={ "command" = "/bin/sh""#),
            "{arguments:?}"
        );
        for secret in [SECRET, BOUND, "fallback-marker"] {
            assert!(!arguments[1].contains(secret), "{arguments:?}");
        }
        assert!(
            arguments[1].contains(r#""env_vars" = ["ABNEGATE_MCP_TEST_0", "ABNEGATE_MCP_TEST_1"]"#),
            "{arguments:?}"
        );
        assert!(
            placeholders
                .secrets
                .iter()
                .any(|secret| secret.expose() == BOUND)
        );
    }

    /// A server given no variable and referring to none starts as
    /// configured, and is handed nothing.
    #[test]
    fn a_plain_stdio_server_starts_as_configured() {
        let server = McpServer::command("uvx", ["mcp-server-appwrite", "--read-only"])
            .with_working_directory("/srv/appwrite")
            .with_tools(["list_projects"]);
        let mut placeholders = Placeholders::under("TEST");

        let arguments = CodexServer::new(&server).arguments("appwrite", &mut placeholders);

        assert_eq!(
            arguments[1],
            r#"mcp_servers.appwrite={ "command" = "uvx", "args" = ["mcp-server-appwrite", "--read-only"], "cwd" = "/srv/appwrite", "enabled_tools" = ["list_projects"], "default_tools_approval_mode" = "approve" }"#
        );
        assert!(placeholders.environment.is_empty());
        assert!(placeholders.templates.is_empty());
    }

    /// A remote server's header reaches Codex through a generated variable
    /// its `env_http_headers` names, resolved against its own secrets alone.
    #[test]
    fn a_remote_servers_headers_are_named_by_generated_variables() {
        let server = McpServer::remote("https://mcp.linear.app/mcp")
            .with_header("Authorization", "Bearer ${LINEAR_TOKEN}")
            .with_header("X-Empty", "")
            .with_secret("LINEAR_TOKEN", BOUND);
        let mut placeholders = Placeholders::under("TEST");

        let arguments = CodexServer::new(&server).arguments("linear", &mut placeholders);

        assert_eq!(
            arguments[1],
            r#"mcp_servers.linear={ "url" = "https://mcp.linear.app/mcp", "env_http_headers" = { "Authorization" = "ABNEGATE_MCP_TEST_0" }, "http_headers" = { "X-Empty" = "" }, "default_tools_approval_mode" = "approve" }"#
        );
        assert_eq!(
            placeholders
                .environment
                .get("ABNEGATE_MCP_TEST_0")
                .map(|value| value.expose().to_string()),
            Some(format!("Bearer {BOUND}"))
        );
    }

    /// `codex exec` has nobody to ask, so every server's calls are approved
    /// in advance by default and decided by whoever serves them, whatever
    /// else the server sets.
    #[test]
    fn every_server_is_approved_in_advance() {
        for server in [
            McpServer::command("uvx", ["server"]),
            McpServer::command("uvx", ["server"]).with_environment("KEY", SECRET),
            McpServer::remote("https://mcp.example.com/mcp"),
            McpServer::remote("https://mcp.example.com/mcp")
                .with_tools(["a"])
                .exclusive()
                .require()
                .eager(),
        ] {
            let mut placeholders = Placeholders::under("TEST");
            let arguments = CodexServer::new(&server).arguments("host", &mut placeholders);
            assert_eq!(
                arguments[1]
                    .matches(r#""default_tools_approval_mode" = "approve""#)
                    .count(),
                1,
                "{arguments:?}"
            );
        }
    }

    /// A supervised server is given no approval mode, so Codex decides each
    /// call to its tools by its own rules, and nothing else it sets changes.
    #[test]
    fn a_supervised_server_is_never_approved_in_advance() {
        let server = McpServer::remote("http://127.0.0.1:9/mcp")
            .with_tools(["a"])
            .exclusive()
            .require()
            .eager()
            .supervise();
        let mut placeholders = Placeholders::under("TEST");

        let arguments = CodexServer::new(&server).arguments("host", &mut placeholders);

        assert_eq!(
            arguments[1],
            r#"mcp_servers.host={ "url" = "http://127.0.0.1:9/mcp", "enabled_tools" = ["a"], "required" = true, "omit_tools_from" = ["deferred"] }"#
        );

        let stdio = McpServer::command("uvx", ["server"])
            .with_environment("KEY", SECRET)
            .supervise();
        let arguments = CodexServer::new(&stdio).arguments("tools", &mut placeholders);
        assert!(
            !arguments[1].contains("default_tools_approval_mode"),
            "{arguments:?}"
        );
    }

    /// Each option becomes a key of its own, timeouts in whole
    /// seconds rounded up, and none appears unless it is set.
    #[test]
    fn the_options_set_in_code_reach_codex_as_keys_of_their_own() {
        let server = McpServer::remote("http://127.0.0.1:9/mcp")
            .with_tools(["a", "b"])
            .exclusive()
            .with_tool_timeout(Duration::from_secs(1800))
            .with_startup_timeout(Duration::from_millis(29_500))
            .require()
            .eager();
        let mut placeholders = Placeholders::under("TEST");

        let arguments = CodexServer::new(&server).arguments("host", &mut placeholders);

        assert_eq!(
            arguments[1],
            r#"mcp_servers.host={ "url" = "http://127.0.0.1:9/mcp", "enabled_tools" = ["a", "b"], "tool_timeout_sec" = 1800, "startup_timeout_sec" = 30, "default_tools_approval_mode" = "approve", "required" = true, "omit_tools_from" = ["deferred"] }"#
        );

        let plain = McpServer::remote("http://127.0.0.1:9/mcp");
        let arguments = CodexServer::new(&plain).arguments("host", &mut placeholders);
        assert_eq!(
            arguments[1],
            r#"mcp_servers.host={ "url" = "http://127.0.0.1:9/mcp", "default_tools_approval_mode" = "approve" }"#
        );
    }

    /// Codex offers every tool a server lists unless told otherwise, so an
    /// exclusive server naming none is given an empty list, which lets Codex
    /// call none of its tools.
    #[test]
    fn an_exclusive_server_naming_no_tool_lets_codex_call_none_of_them() {
        let mut placeholders = Placeholders::under("TEST");

        let exclusive = McpServer::remote("http://127.0.0.1:9/mcp").exclusive();
        let arguments = CodexServer::new(&exclusive).arguments("host", &mut placeholders);
        assert!(
            arguments[1].contains(r#""enabled_tools" = []"#),
            "{arguments:?}"
        );

        let open = McpServer::remote("http://127.0.0.1:9/mcp");
        let arguments = CodexServer::new(&open).arguments("host", &mut placeholders);
        assert!(!arguments[1].contains("enabled_tools"), "{arguments:?}");
    }

    /// A value Codex reads as TOML holds whatever text a configuration gives
    /// it without ending its string early.
    #[test]
    fn the_options_reach_codex_as_toml_it_cannot_misread() {
        let server = McpServer::remote("http://127.0.0.1:9/mcp\"\\\n\u{7f}")
            .with_tools(["read\"file"])
            .exclusive();
        let mut placeholders = Placeholders::under("TEST");

        let arguments = CodexServer::new(&server).arguments("host", &mut placeholders);

        assert_eq!(
            arguments[1],
            r#"mcp_servers.host={ "url" = "http://127.0.0.1:9/mcp\"\\\n\u007F", "enabled_tools" = ["read\"file"], "default_tools_approval_mode" = "approve" }"#
        );
    }

    #[test]
    fn codex_refuses_what_it_could_only_take_on_its_command_line() {
        for (server, refusal) in [
            (
                McpServer::remote("https://mcp.example.com/sse").with_transport(McpTransport::Sse),
                Refusal::ServerSentEvents,
            ),
            (
                McpServer::remote("https://mcp.example.com/${TOKEN}/mcp")
                    .with_secret("TOKEN", BOUND),
                Refusal::ReferringUrl,
            ),
            (
                McpServer::command("uvx", ["server"]).with_environment("NOT-A-NAME", "value"),
                Refusal::VariableName,
            ),
            (
                McpServer::command("uvx", ["server"]).with_environment("1ST", "value"),
                Refusal::VariableName,
            ),
        ] {
            assert_eq!(
                CodexServer::new(&server).refusal(),
                Some(refusal),
                "{server:?}"
            );
        }
        for server in [
            McpServer::remote("https://mcp.linear.app/mcp").with_transport(McpTransport::Http),
            McpServer::command("uvx", ["server"]).with_environment("_KEY_2", "value"),
        ] {
            assert_eq!(CodexServer::new(&server).refusal(), None, "{server:?}");
        }
    }
}
