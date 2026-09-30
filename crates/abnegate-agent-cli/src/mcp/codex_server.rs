use crate::mcp::inline_table::InlineTable;
use crate::mcp::launch::Launch;
use crate::mcp::placeholders::Placeholders;
use crate::mcp::placeholders::refers;
use crate::mcp::refusal::Refusal;
use crate::mcp::server::McpServer;
use crate::mcp::transport::McpTransport;

const CONFIG: &str = "-c";
const SERVERS: &str = "mcp_servers";
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
        self.server
            .url
            .as_deref()
            .filter(|url| refers(url))
            .map(|_| Refusal::ReferringUrl)
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
        let table = match self.server.tools.is_empty() {
            true => table,
            false => table.with_texts("enabled_tools", &self.server.tools),
        };
        placeholders
            .secrets
            .extend(self.server.secrets.values().cloned());
        [CONFIG.to_string(), format!("{SERVERS}.{name}={table}")]
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
            r#"mcp_servers.appwrite={ "command" = "uvx", "args" = ["mcp-server-appwrite", "--read-only"], "cwd" = "/srv/appwrite", "enabled_tools" = ["list_projects"] }"#
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
            r#"mcp_servers.linear={ "url" = "https://mcp.linear.app/mcp", "env_http_headers" = { "Authorization" = "ABNEGATE_MCP_TEST_0" }, "http_headers" = { "X-Empty" = "" } }"#
        );
        assert_eq!(
            placeholders
                .environment
                .get("ABNEGATE_MCP_TEST_0")
                .map(|value| value.expose().to_string()),
            Some(format!("Bearer {BOUND}"))
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
