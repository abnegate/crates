# abnegate-agent-cli

Coding agent CLIs driven as child processes, behind the same
`abnegate_llm::CompletionProvider` contract as an HTTP model. `CliProvider` runs
`claude --output-format stream-json` or `codex exec --json` with the
conversation flattened into one prompt on stdin, reads the agent's
newline-delimited JSON as it streams with a cap on any one event, and returns its
final prose as a completion; `CliProvider::execute` returns the whole
`Execution` for a caller that needs the structured answer, the session to
resume, the cost, or the run's log files. A Claude or Codex run can attach MCP
servers, and a Claude run can restrict its tools, or be confined to reading its
working directory, and a run
that fails in any way takes every process the agent forked with it. The agent is
given an allowlisted part of this process's environment, the names
`CliSettings::allow` adds (a proxy among them with
`CliSettings::with_proxy_variables`) and what the settings hand it, or this
process's whole environment with `CliSettings::inherit_environment`. A name the
agent signs in with (`AgentKind::credentials`) passes only when the credential is
inherited, however `CliSettings::allow` names it. Every value it is handed but
the allowlisted names, its configuration variables, the public variables, the
proxy bypass list and what it inherits is scrubbed from what the run writes
down, as written, JSON-escaped or percent-encoded; a secret the agent re-encodes
any other way is not recognised. Unix only.

The agent's own tools, and every stdio MCP server it starts, can read
everything it is given. A remote MCP server is sent nothing the agent is given
by name: each `${VAR}` in its URL and headers resolves only to a secret bound to
that server with `McpServer::with_secret`, or by name with
`McpConfig::with_secret`, or else to its default, and a server that refers to a
variable with neither is left out. A stdio server's references read the secrets
bound to it first, and then what the agent is given and this process's
environment. The file the CLI reads holds no environment or header value and no
bound secret: each reaches the agent in a generated variable of its environment,
which its tools and stdio servers can read too, so binding keeps a secret from
every other remote server, not from them.

No agent is passed a permission-bypass flag, and nothing is approved in advance
but what the settings allow. Claude's own tools keep Claude's own permission
checks. Codex's do not: `codex exec` never asks for approval, so only its
sandbox confines them, whichever one the user's configuration or a `--sandbox`
argument names, and `danger-full-access` confines nothing.

## Claude turns

A Claude run reads its stream through one `Turn` (from `AgentKind::turn`), since
a line's meaning can hang on the lines before it. Claude reports a refused plan
window on a `rate_limit_event` that names no agent, so the window is remembered
and fails the turn, as `rate limit reached: {report}: <Claude's words>`, only
when the main agent's next line is a failed request or the result a failure. A
stream that ends on the window alone still fails with it. A request the
account's usage credits carry past the window is headroom, and the turn is
logged once at info and named in `StdoutParseResult::credits`. A model or a
context the account cannot fund fails in Claude's own words after
`CREDITS_REQUIRED` or `LONG_CONTEXT_CREDITS_REQUIRED`, or after
`CREDITS_UNCONFIRMED` when Claude could not look the credits up. A subagent's
lines contribute only the tools it calls: its words, token counts and refusals
stay in its own conversation, and the main agent answers without them. Nothing
names a failure by the words it is written in.

## Failures

A failure the agent reported is its own words, then `STDERR_HEADING`, then the
last whole lines of its stderr that fit in 1 KiB: some agents give the reason
for a failure, such as a sign-in that could not be renewed, only on stderr.
Split on `STDERR_HEADING` to read the agent's words alone. An agent that exits
nonzero with no report of its own fails with the end of its stderr. Stderr is
read to its end, every line of it checked against the tripwire, and only its
last `output_limit` bytes are kept in `Execution::stderr`. A run stopped for
prose past its output limit fails with a message starting with
`PROSE_EXCEEDED`, so a caller can tell it from any other failure.

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
