# abnegate-agent

The parts an LLM agent is built from: a tool registry the model acts through, a
ReAct loop that drives it, and the context, history and session handling a long
conversation needs. `ToolRegistry` holds file, command and background-job tools,
each declaring its own tier; file tools stay beneath the working directory, and
commands run in a process group of their own that sees only the allowlisted
environment the `ToolContext` names. `Agent` runs the loop over any
`abnegate_llm::CompletionProvider` (ask the model, run the tools it calls, feed
their results back, until it answers), and a call whose tier needs confirming
runs only once `AgentCallback::approve` allows it, which by default it does not.
Each request goes through `context::prepare`, which folds consumed history into
a checkpoint when the model's context would overflow, without ever editing the
history. `chat` is the storage boundary a multi-turn chat needs, `session` saves
and reloads agent runs, and `template` renders `{{key}}` prompt templates.

## Features

- `mcp`: an MCP client that launches stdio servers and adds their tools to a `ToolRegistry`. Its configuration is `abnegate-agent-cli`'s `McpConfig`, re-exported here, so one `mcp.json` drives this client and a coding agent CLI alike.

## Usage

```sh
cargo add abnegate-agent abnegate-llm
```

```rust,no_run
use std::sync::Arc;

use abnegate_agent::Agent;
use abnegate_agent::AgentConfig;
use abnegate_agent::NoOpCallback;
use abnegate_agent::RunError;
use abnegate_agent::ToolContext;
use abnegate_agent::ToolRegistry;
use abnegate_llm::Credential;
use abnegate_llm::HttpProvider;

async fn list() -> Result<(), RunError> {
    let provider = HttpProvider::connect(
        "gateway",
        "http://127.0.0.1:4000/v1",
        &Credential::Inherited,
        "qwen3",
    );
    let agent = Agent::new(
        Arc::new(provider),
        "qwen3",
        ToolRegistry::with_defaults(),
        AgentConfig::default(),
        ToolContext::default(),
    );

    let state = agent.run("List the files in src.", &NoOpCallback).await?;
    println!("{:?}", state.final_response);
    Ok(())
}
```

`NoOpCallback` approves nothing that needs confirming: the model can read, list
and search, and any write or command it asks for is refused.

The model is named when the agent is built, so one provider can serve several
agents. When the provider stops on custom sequences or asks for reasoning, give
compaction a provider that does neither with `Agent::with_summarizer`: a summary
is a structured rewrite, asked for at temperature 0.

## MCP

With the `mcp` feature, `McpConfig::from_environment` reads the servers an
application configured under its own prefix: `ACME_MCP_SERVERS` inline,
`ACME_MCP_CONFIG` naming a file, or `~/.acme/mcp.json`, each in the
`mcpServers` shape. `mcp::with_defaults_and_mcp` launches every enabled command
server and adds its tools to the default registry as `server__tool`: only the
tools its `tools` list names, by the names a CLI gives them, when it has one. A
server marked `"disabled": true`, one reached by `url`, which only a CLI
attaches, or one that is not valid is skipped, and an entry that cannot be read
is skipped with a warning while the rest load. Each `${VAR}` or
`${VAR:-default}` in a server's command, arguments and `env` is expanded from
this process's environment first, as Claude Code expands it. Each server's
child sees only the allowlisted environment plus its own `env`, unless it sets
`inherit_environment`, and starts in its `cwd` when it names one.

This example needs the `mcp` feature, so it is not compiled with this README;
the same example is compiled in the `mcp` module's documentation.

```rust,ignore
use abnegate_agent::McpConfig;
use abnegate_agent::McpServer;
use abnegate_agent::mcp::with_defaults_and_mcp;

async fn tools() {
    let config = McpConfig::from_environment("ACME")
        .fallback("notes", McpServer::command("notes-server", ["mcp"]));
    let registry = with_defaults_and_mcp(&config).await;
    println!("{:?}", registry.names());
}
```
