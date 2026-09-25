# abnegate-agent-cli

Coding agent CLIs driven as child processes, behind the same
`abnegate_llm::CompletionProvider` contract as an HTTP model. `CliProvider` runs
`claude --output-format stream-json` or `codex exec --json` with the
conversation flattened into one prompt on stdin, reads the agent's
newline-delimited JSON as it streams with a cap on any one event, and returns its
final prose as a completion; `CliProvider::execute` returns the whole
`Execution` for a caller that needs the structured answer, the session to
resume, the cost, or the run's log files. A Claude run can attach MCP servers,
restrict its tools, or be confined to reading its working directory, and a run
that fails in any way takes every process the agent forked with it. The agent is
given an allowlisted part of this process's environment, the names
`CliSettings::allow` adds (a proxy among them with
`CliSettings::with_proxy_variables`) and what the settings hand it, or this
process's whole environment with `CliSettings::inherit_environment`. Every value
it is handed but the allowlisted names, its configuration variables, the public
variables, the proxy bypass list and what it inherits is scrubbed from what the
run writes down, as written, JSON-escaped or percent-encoded; a secret the agent
re-encodes any other way is not recognised. Unix only.

The agent's own tools, and every stdio MCP server it starts, can read
everything it is given. A remote MCP server is sent nothing the agent is given
by name: each `${VAR}` in its URL and headers resolves only to a secret bound to
that server with `McpServer::with_secret`, or by name with
`McpConfig::with_secret`, or else to its default, and a server that refers to a
variable with neither is left out. The file the CLI reads holds no environment
or header value and no bound secret: each reaches the agent in a generated
variable of its environment, which its tools and stdio servers can read too, so
binding keeps a secret from every other remote server, not from them.

## Features

None.

## Usage

```sh
cargo add abnegate-agent-cli abnegate-llm
```

```rust,no_run
use abnegate_agent_cli::AgentKind;
use abnegate_agent_cli::CliProvider;
use abnegate_agent_cli::CliSettings;
use abnegate_llm::CompletionProvider;
use abnegate_llm::CompletionRequest;
use abnegate_llm::Message;
use abnegate_llm::ProviderError;
use abnegate_llm::RequestOptions;

async fn explain() -> Result<(), ProviderError> {
    let settings = CliSettings::default()
        .with_working_directory("/path/to/repository")
        .read_only();
    let provider = CliProvider::agent(AgentKind::Claude, settings);

    let messages = [Message::user("What does main.rs do?")];
    let request = CompletionRequest::new("sonnet", &messages, RequestOptions::new(1024));
    let completion = provider.complete(request).await?;
    println!("{:?}", completion.message.content);
    Ok(())
}
```
