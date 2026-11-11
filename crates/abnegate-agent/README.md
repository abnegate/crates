# abnegate-agent

The parts an LLM agent is built from: a tool registry the model acts through, a
ReAct loop that drives it, and the context, history and session handling a long
conversation needs. `ToolRegistry` holds file, command and background-job tools,
each declaring its own tier; file tools stay beneath the working directory
unless the context is unrestricted, and never reach what it denies. Commands run
in a process group of their own that sees only the allowlisted environment the
`ToolContext` names. `Agent` runs the loop over any
`abnegate_llm::CompletionProvider` (ask the model, run the tools it calls, feed
their results back, until it answers), and a call whose tier needs confirming
runs only once `AgentCallback::approve` allows it, which by default it does not.
Each request goes through `context::prepare`, which folds consumed history into
a checkpoint when the model's context would overflow, without ever editing the
history. `chat` is the storage boundary a multi-turn chat needs, `session` saves
and reloads agent runs, and `template` renders `{{key}}` prompt templates.

## Features

- `mcp`: an MCP client that launches stdio servers and connects to streamable HTTP ones, and adds their tools to a `ToolRegistry`. Its configuration is `abnegate-agent-cli`'s `McpConfig`, re-exported here, so one `mcp.json` drives this client and a coding agent CLI alike.

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

A search or a listing walks for the context's `search_timeout`, 20 seconds
unless `ToolContext::with_search_timeout` sets another, and then reports what
it found so far, marked as stopped early. Once its caller gives up on it, a
walk stops at its next entry rather than reading on to its own limit.
`search_code` shows a match only when the file it is in lies beneath the
searched directory and, once its links are resolved, stays beneath the working
directory unless the context is unrestricted.

A registry without `wait_for`, such as one serving a coding agent whose own
loop cannot park on it, serves each tool that tells the model to wait in its
`Tool::unwaited` form: the shell tools' schemas, spawn receipts and sleep
refusals then say that waiting needs a tool the turn does not have. A registry
that has `wait_for` serves every tool as it is, and `job::parse_receipt` reads a
receipt back in either form.

The model is named when the agent is built, so one provider can serve several
agents. When the provider stops on custom sequences or asks for reasoning, give
compaction a provider that does neither with `Agent::with_summarizer`: a summary
is a structured rewrite, asked for at temperature 0.

## Withheld paths

`ToolContext::with_denied` names paths no file tool reads, lists, searches or
writes beneath, even in an unrestricted context, and that `run_command` and
`run_shell` refuse as a `cwd`. A relative entry is taken from the working
directory. Each is compared by what it is on disk rather than by how a path
spells it: every link on the way is followed where the kernel would follow it,
a dangling one included, and each directory is matched by its device and
inode, so another case, another Unicode normalization, a firmlink or a bind
mount reaches nothing. A denied path nobody has made yet is matched by the
names it will have, case-folded, from the deepest directory that exists. A
walk shows a denied directory and never enters it. Every process's `/proc`
entry, the reader's own descriptors under `/dev/fd` and macOS's `/.vol` are
withheld from every context the same way. A refused call answers
`tool::OFF_LIMITS`.

```rust
use abnegate_agent::ToolContext;

let mut context = ToolContext::default()
    .within("/srv/checkout")
    .with_denied(["/srv/state", ".env"]);
context.unrestricted = true;
```

## MCP

With the `mcp` feature, `McpConfig::from_environment` reads the servers an
application configured under its own prefix: `ACME_MCP_SERVERS` inline,
`ACME_MCP_CONFIG` naming a file, or `~/.acme/mcp.json`, each in the
`mcpServers` shape. `mcp::with_defaults_and_mcp` launches every enabled command
server, connects to every enabled server reached by `url` over streamable HTTP,
and adds its tools to the default registry as `server__tool`: only the tools its
`tools` list names, by the names a CLI gives them, when it has one. A server
marked `"disabled": true`, one of `"type": "sse"`, which this client does not
speak, or one that is not valid is skipped, and an entry that cannot be read is
skipped with a warning while the rest load; `McpHub::outcomes` says what became
of each. Each `${VAR}` in a server reached by `url` resolves, here and in
`abnegate-agent-cli`'s `CliProvider` alike, against the secrets bound to that
server in code with `McpServer::with_secret` or `McpConfig::with_secret`, which
a document never holds, or else its default, and never against this process's
environment or the one a CLI is given. Such a server is never sent to a
link-local or cloud metadata address (behind a proxy, which resolves names
itself, only a literal one is refused), follows only a same-origin 307 or 308
redirect, and is reached through the hub's proxy alone. Releases before this
one skipped every server reached by `url`; `with_defaults_and_mcp` and
`McpHub::connect` now contact each enabled one, so mark a server that must not
be contacted `"disabled": true`, or call `McpServer::disable` on it. Each `${VAR}` or
`${VAR:-default}` in a server's command, arguments and `env` is expanded before
it starts, as Claude Code expands it: from the secrets bound to the server with
`McpServer::with_secret` or `McpConfig::with_secret` first, and then from this
process's environment. Each server's
child sees only the allowlisted environment plus its own `env`, unless it sets
`inherit_environment`, and starts in its `cwd` when it names one. The proxy
`ABNEGATE_EXEC_PROXY_URL` names is laid over that before the handshake, or the
one a host hands `McpHub::connect_through`, as `ToolContext::with_proxy` hands
one to every command and background job a tool starts.

A server that was not connected is `McpOutcome::Failed`, which names its kind
as an `McpFailure` and carries the HTTP status of the answer that failed it, so
a host can say why without reading the failure's text or showing what the
server sent. The status is set exactly when the request that failed the
connection got an answer that was not 2xx, from the server or a proxy in front
of it, and the client saw it; it is `None` for a 2xx the client could not use,
for a request that got no answer, and for every failure before a request is
sent. The client does not see the answer to the last attempt it makes after
retrying a request an HTTP/2 server refused, so that answer can leave `None`
too, and one such as a bare 401 or 403 then fails as `McpFailure::Protocol`.
An `https` server behind a proxy is reached through a tunnel, and the client
never sees the proxy's refusal of that tunnel, a 407 included, as a response,
so it is reported as `McpFailure::Unreachable` with no status.

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
