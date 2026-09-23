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
given only an allowlisted part of this process's environment, and every secret
it is handed is scrubbed from what the run writes down, as written,
JSON-escaped or percent-encoded; a secret the agent re-encodes any other way is
not recognised. Unix only.

## Features

None.

## Usage

```sh
cargo add abnegate-agent-cli abnegate-llm
```

```rust,no_run
use abnegate_agent_cli::{AgentKind, CliProvider, CliSettings};
use abnegate_llm::{CompletionProvider, CompletionRequest, Message, ProviderError, RequestOptions};

async fn explain() -> Result<(), ProviderError> {
    let settings = CliSettings::default()
        .with_working_directory("/path/to/repository")
        .read_only();
    let provider = CliProvider::agent(AgentKind::Claude, settings);

    let messages = [Message::user("What does main.rs do?")];
    let request = CompletionRequest::new("sonnet", &messages, RequestOptions { reserved: 1024 });
    let completion = provider.complete(request).await?;
    println!("{:?}", completion.message.content);
    Ok(())
}
```
