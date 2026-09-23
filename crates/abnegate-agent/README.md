# abnegate-agent

The parts an LLM agent is built from: a tool registry the model acts through, a
ReAct loop that drives it, and the context, history and session handling a long
conversation needs. `ToolRegistry` holds file, command and background-job tools,
each declaring its own tier; file tools stay beneath the working directory, and
commands run in a process group of their own that sees only the allowlisted
environment the `ToolContext` names. `Agent` runs the loop (ask the model, run
the tools it calls, feed their results back, until it answers), and a call whose
tier needs confirming runs only once `AgentCallback::approve` allows it, which by
default it does not. Each request goes through `context::prepare`, which folds
consumed history into a checkpoint when the model's context would overflow,
without ever editing the history. `chat` is the storage boundary a multi-turn
chat needs, `session` saves and reloads agent runs, and `template` renders
`{{key}}` prompt templates.

## Features

- `mcp`: an MCP client that launches stdio servers and adds their tools to a `ToolRegistry`, configured from the environment under an application's own prefix.

## Usage

```sh
cargo add abnegate-agent abnegate-llm
```

```rust,no_run
use abnegate_agent::{Agent, AgentConfig, AgentError, NoOpCallback, ToolContext, ToolRegistry};
use abnegate_llm::{LlmClient, LlmConfig};

async fn list() -> Result<(), AgentError> {
    let llm = LlmClient::new(LlmConfig::new("http://127.0.0.1:4000/v1", "qwen3", ""));
    let tools = ToolRegistry::with_defaults();
    let agent = Agent::new(llm, tools, AgentConfig::default(), ToolContext::default());

    let state = agent.run("List the files in src.", &NoOpCallback).await?;
    println!("{:?}", state.final_response);
    Ok(())
}
```

`NoOpCallback` approves nothing that needs confirming: the model can read, list
and search, and any write or command it asks for is refused.
