# abnegate-agent-cli

Coding agent CLIs driven as child processes, behind the same
`abnegate_llm::CompletionProvider` contract as an HTTP model. `CliProvider` runs
`claude --output-format stream-json` or `codex exec --json` with the
conversation flattened into one prompt on stdin, reads the agent's
newline-delimited JSON as it streams with a cap on any one event, and returns its
final answer as a completion; `CliProvider::execute` returns the whole
`Execution` for a caller that needs the structured answer, the session to
resume, the cost, or the run's log files, and `CliProvider::stream` yields the
run's `AgentEvent`s as the agent emits them, ending in the error `complete`
would have returned when the run failed. `CliProvider::execute_prompt` and
`CliProvider::stream_prompt` write a prompt's bytes to stdin unchanged, with no
role label, trimming or escaping, so Claude reads a leading slash command such
as `/issue ENG-42` as one; text the caller does not control belongs in a
conversation, whose role labels are escaped. A conversation's images reach
Claude in a stream-json message and Codex as files (see [Images](#images)). A run can start the agent's own
`Session` under an id of the caller's choosing, or resume one, and
`Session::carry` moves a session's file into another home. A Claude or Codex
run can attach MCP servers and withhold the agent's own tools, and a Claude run
can restrict its tools, be confined to reading its working directory, or be
isolated from what the host configured for it. A run that fails in any way
takes every process the agent forked with it. The agent is
given an allowlisted part of this process's environment and its locale
categories (`LC_CTYPE`, `LC_TIME` and the rest), the names
`CliSettings::allow` adds (a proxy among them with
`CliSettings::with_proxy_variables`) and what the settings hand it, or this
process's whole environment with `CliSettings::inherit_environment`. A name the
agent signs in with (`AgentKind::credentials`) passes only when the credential is
inherited, however `CliSettings::allow` names it, and never, nor a cloud
sign-in in `CLOUD_CREDENTIALS` nor its configuration variables unless allowed,
under `CliSettings::withhold_host_credentials`. Every value it is handed but
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
but what the settings allow and the tools of the MCP servers a Codex run is
given, which `codex exec` has nobody to ask about: every call to an attached
server's tools is approved with its `default_tools_approval_mode = "approve"`,
unless the server is supervised (`McpServer::supervise`, or `"supervised": true`
in a configuration document), which leaves each call to Codex's own approval
rules as before. Claude's own tools keep
Claude's own permission checks. Codex's do not: `codex exec` never asks for
approval, so only its sandbox confines them, whichever one
`CliSettings::with_sandbox`, the user's configuration or a `--sandbox` argument
names, and `danger-full-access` confines nothing.

## Models and effort

`AgentKind::models` lists the models offered for an agent, in the order it
ranks them, and `AgentKind::named_only` the ones among them that run on usage
credits beyond the plan's allowance, to run only when a person names one.
`AgentKind::knows` tells whether an agent can run a model: one of Claude's
aliases or a full `claude-` name, or one of Codex's offered models or a `gpt-`
name. A run is still given whatever model its caller names, since a Codex run
can be pointed at a local model, so a host that offers only what the agent
knows checks a model with it first. `AgentKind::label` is how a model is shown
to a person.

`CliSettings::with_effort` sets how hard the agent thinks, as Claude's
`--effort` or Codex's `-c model_reasoning_effort="<effort>"`. It is passed only
when the run's model takes it, as `AgentKind::efforts` lists, so
`ReasoningEffort::Auto`, `ReasoningEffort::Off`, and any level on a model whose
efforts are not known leave it to the agent. A run given an effort refuses a
caller argument that would set another.

```rust,no_run
use abnegate_agent_cli::AgentKind;
use abnegate_agent_cli::CliProvider;
use abnegate_agent_cli::CliSettings;
use abnegate_llm::ReasoningEffort;

let model = "opus";
assert!(AgentKind::Claude.knows(model));
let settings = CliSettings::default().with_effort(ReasoningEffort::High);
let provider = CliProvider::agent(AgentKind::Claude, settings);
```

## Sign-in variables

Besides the `abnegate_exec::DEFAULT_ENVIRONMENT` names and the locale
categories, the agent inherits `SHELL`, `XDG_RUNTIME_DIR`, `SSL_CERT_DIR` and
`NODE_EXTRA_CA_CERTS` from this process. `CliSettings::refuse_credentials`
refuses every variable any agent signs in with (`AgentKind::credentials`),
however `CliSettings::allow` names it or `CliSettings::inherit_environment`
passes it, and a stdio MCP server's reference to one reads as set but empty,
unless a secret is bound to the server under that name. Only the agent's own
sign-in still passes, when its credential is inherited and the host's are not
withheld. It is off by default, since a caller may allow another agent's key on
purpose, for a stdio MCP server the agent starts, say; a host that hands every
run its sign-in itself, and passes an operator's list of variables through,
turns it on.

## Cleanup

The agent leads a process group of its own. Once it exits, or is stopped, the
run kills what is left of the group while the agent is still unreaped, so the
kill never reaches a group that took over its id, and reaps it only then. A
run that fails, times out or is dropped takes the whole group with it. A
successful run leaves what the agent started running, such as a server it
started for the caller, unless it still holds the agent's output open past the
grace period, or `CliSettings::with_cleanup(Cleanup::Always)` asks for every
run to be cleaned up; success here means the agent's stream
finished its turn without a failure, nothing stopped the agent, and the agent
exited with status zero, which the run reads before it reaps the agent.

`CliSettings::with_marker` names an environment variable each run sets to a
value of its own. Everything the agent starts inherits it, so a cleanup also
finds and stops what left the group, such as a daemon in a session of its own:
SIGTERM, `GRACE_PERIOD` to exit, then SIGKILL, on the runtime's blocking pool,
and a dropped run sends SIGKILL at once. On macOS a sweep cannot see Apple's
platform binaries, whose environment is hidden: a `/bin/sh`, `zsh`, `bash` or
`sleep` that left the group is never found, though what such a shell starts
from outside the system is. The run's timeout bounds only the
agent, never the sweep. `CliSettings::with_registry` registers each run's group,
and tracks its marker, in an `abnegate_exec::GroupRegistry` such as
`GroupRegistry::global()`, so a host can pass a Ctrl-C on to every run with
`interrupt_all` and stop them all as it shuts down with `shutdown`; a registry
that has been killed refuses to start a run.

```rust,no_run
use abnegate_agent_cli::Cleanup;
use abnegate_agent_cli::CliSettings;
use abnegate_exec::GroupRegistry;

let settings = CliSettings::default()
    .with_cleanup(Cleanup::Always)
    .with_marker("MY_APPLICATION_RUN")
    .with_registry(GroupRegistry::global());
```

## Tools and isolation

`CliSettings::withhold_tools` takes the agent's own tools away, for a host that
serves it every tool it may use over MCP and decides each call where it runs:
Claude is given `--tools ""` and loads only the attached servers, and Codex
loads no user configuration, plugin or app, runs without its shell, image,
goal, sleep, subagent, web-search and question tools, and runs in the
`read-only` sandbox (`-c sandbox_mode="read-only"` on a resumed session). It is
refused beside `CliSettings::read_only`, any other sandbox, the web or a
permission that is not an MCP tool's. Every caller argument off an allowlist
is refused too: Claude passes only what a read-only run passes, and Codex only
flags such as `--ephemeral`, `--oss` or `--output-schema` and a `-c` override
of the model, its provider or how its reasoning reads, never `--enable`,
`--profile`, `--sandbox` or a `-c` override of `features` or `mcp_servers`.

`CliSettings::with_permission_prompt(server, tool)` sends each question Claude
would ask before using one of its own tools to an attached server's tool, with
`--permission-prompt-tool`, so the host answers it under its own approval
rules. `AgentKind::tool` spells the name a model calls such a tool by,
`mcp__<server>__<tool>`.

`CliSettings::isolate` runs Claude apart from whatever the host configured for
it: no settings file loads (`--setting-sources ""`), no other session may
message it, and it is given `DISABLE_AUTOUPDATER=1`,
`CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1` and
`CLAUDE_CODE_DISABLE_AUTO_MEMORY=1` unless `CliSettings::with_variable` says
otherwise. An isolated read-only run loads no settings at all, where it would
load the user's, and keeps every flag that confines it. Codex ignores it, so a
host can set it on every run.

An `McpServer` also takes options Claude's own MCP entry has no key for, which
never reach Claude's file or a log: `with_tool_timeout` (Codex's
`tool_timeout_sec`, and for Claude the longest of the run's servers as
`MCP_TOOL_TIMEOUT` and `CLAUDE_CODE_MCP_TOOL_IDLE_TIMEOUT`),
`with_startup_timeout`, `require`, `eager` and `supervise` (Codex only), and
`exclusive`, which holds a server to exactly the tools it names, none when it
names none. A configuration document carries each only when it is set:
`tool_timeout_sec` and `startup_timeout_sec` as a number of seconds (also read
as `tool_timeout_seconds` and `startup_timeout_seconds`), and `required`,
`eager`, `exclusive` and `supervised` as `true`, so a server written out and
read back keeps every option.

```json
{
  "mcpServers": {
    "host": {
      "type": "http",
      "url": "http://127.0.0.1:8421/mcp",
      "tools": ["read_file", "run_command"],
      "exclusive": true,
      "tool_timeout_sec": 1800,
      "startup_timeout_sec": 30,
      "required": true,
      "eager": true,
      "supervised": true
    }
  }
}
```

```rust
use std::time::Duration;

use abnegate_agent_cli::AgentKind;
use abnegate_agent_cli::CliSettings;
use abnegate_agent_cli::McpServer;

# let token = String::new();
let host = McpServer::remote("http://127.0.0.1:8421/mcp")
    .with_header("Authorization", "Bearer ${HOST_MCP_TOKEN}")
    .with_secret("HOST_MCP_TOKEN", token)
    .with_tools(["read_file", "run_command"])
    .exclusive()
    .with_tool_timeout(Duration::from_secs(1800))
    .with_startup_timeout(Duration::from_secs(30))
    .require()
    .eager();
let settings = CliSettings::default()
    .isolate()
    .with_mcp_server("host", host)
    .with_permission_prompt("host", "approve_tool_call");
assert_eq!(
    AgentKind::Claude.tool("host", "read_file"),
    "mcp__host__read_file"
);
```

## Claude turns

A Claude run reads its stream through one `Turn` (from `AgentKind::turn`), since
a line's meaning can hang on the lines before it. Claude reports a refused plan
window on a `rate_limit_event` that names no agent, so the window is remembered
and limits the turn, as `rate limit reached (<window>, <status>): <Claude's
words>`, only when the main agent's next line is a failed request or the result
a failure. A stream that ends on the window alone, with no line after it, still
fails with it, as `ProviderError::Agent` in the same words rather than as a
limit. A request
the account's usage credits carry past the window is headroom, and the turn is
logged once at info and named in `StdoutParseResult::credits`. A model or a
context the account cannot fund is limited in Claude's own words after
`CREDITS_REQUIRED` or `LONG_CONTEXT_CREDITS_REQUIRED`, or fails after
`CREDITS_UNCONFIRMED` when Claude could not look the credits up. A failed
result with status 429, or in a limit's words, is a limit too, unless its
status is 401. A subagent's words, token counts and refusals stay in its own
conversation, and the main agent answers without them; the calls it makes are
steps of its run rather than the main agent's tools (see
[Tool output and subagents](#tool-output-and-subagents)).

A successful result's text is the turn's final answer, `AgentEvent::Answer`,
yielded just before `AgentEvent::Finished` and kept in
`StdoutParseResult::answer` apart from the narration streamed as
`AgentEvent::Text` on the way there. `StdoutParseResult::reply` is that answer,
or all the prose when the agent gave none, and is what `complete` returns. A
failed result, such as `error_max_turns`, fails in its words and then the
`errors` it lists, a line apiece, and in its subtype only when it gave neither.

## Prose and activity

Each block of an agent's prose after the first, an `AgentEvent::Text`, starts
with a paragraph break, `\n\n`, so the blocks read apart when they are joined,
as the prose a run folds is; the final answer, `AgentEvent::Answer`, is left as
the agent gave it. `activity` is the markdown list item a reader can be shown
for a tool the agent ran for itself: its name and, in a code span, the first
line of what the call was about, sanitized of control sequences and
credentials and cut to 80 characters. A call whose name starts with one of the
hidden prefixes, such as the tools of an MCP server the host renders itself,
has no line.

```rust
use abnegate_agent_cli::activity;
use abnegate_llm::ToolCall;

let call = ToolCall::function("toolu_01", "Bash", r#"{"command":"git status"}"#);
assert_eq!(activity(&call, &[]).as_deref(), Some("- Bash `git status`\n"));
```

## Tool output and subagents

What one of the agent's own tool calls printed, once the call finishes, is an
`AgentEvent::Output`: the call, the text, and whether it worked (a Claude tool
result not marked an error, a Codex command that exited zero). News of a
subagent the agent started for itself, through Claude's Agent tool (Task in
older releases) or Codex's collaboration tools, is an `AgentEvent::Subagent`,
one `Subagent` update per line that says something new about it: its start,
its progress and model, each of its own calls as a `SubagentStep`, and how it
ended, as a `SubagentStatus`. A field an update leaves `None` did not change. A
background subagent's news can come after the event that ends the turn.

Both are yielded by `CliProvider::stream` only. Neither is folded into a
`StdoutParseResult`, and neither counts toward the output limit, which
measures the agent's prose. Every word they carry, a tool's output, a
subagent's description, summary, error and each step's subject, is sanitized
and scrubbed of the run's secrets before it is yielded. Nothing renders them:
a caller that lists a subagent's calls as it lists the main agent's renders
each step itself, with `Subagent::activity` for instance, and counts it toward
whatever budget it keeps.

A subagent's own calls, and those of every agent nested inside it, are steps
of the subagent the main agent started, so they are no longer
`AgentEvent::Tool` events and no longer appear in `StdoutParseResult::tools`,
which holds the main agent's calls alone. A call by an agent no known subagent
descends from, such as a forked skill's, stays the main agent's.

```rust
use abnegate_agent_cli::AgentEvent;

fn shown(event: &AgentEvent) -> Option<String> {
    match event {
        AgentEvent::Subagent(update) => update.activity(&["mcp__host__"]),
        AgentEvent::Output(output) if !output.succeeded => {
            Some(format!("{} failed\n", output.call.function.name))
        }
        _ => None,
    }
}
# let _ = shown;
```

## Limits and windows

A turn a usage limit refused is `AgentEvent::Limited`, never
`AgentEvent::Failed`, and the run fails with `ProviderError::Limited`: it
renders exactly as the agent failure it replaces, the end of stderr included,
and its `Limit` says when the limit lifts, which window refused the turn, and
whether the account's usage credits ran out. `CliProvider::stream` ends with
that error rather than yielding the event, and `complete` reports the same
limit. `execute`'s `Execution::stdout.limit` carries the agent's words alone;
the end of its stderr is in `Execution::stderr`. A Codex turn that fails in a
limit's words is a limit unless they name a 401. While a Claude turn runs, a
report of how much of a plan's window is spent is an `AgentEvent::Window`, under
the name Claude streams it with (`five_hour`, `seven_day`, ...) and its
`WindowKind`, and `StdoutParseResult::windows` keeps the latest report on each.
The window only usage credits extend is never reported as one.

## Context window

A run reports the context window of the model its turn ran on, and how many
tokens of it the turn's last request filled, as `AgentEvent::Occupancy`, kept
in `StdoutParseResult::occupancy`. Claude names each model's window in its
result's `modelUsage`: the window is the main agent's model's, filled by input,
cache reads, cache writes and output of its latest request, or else the widest
window the result names, with no fill. It comes with the rest of the result,
ahead of the answer and the event that ends the turn.

Codex writes the window only to the rollout it keeps of the session, under the
`CODEX_HOME` the agent is given, or else `.codex` under its `HOME`. Once an
agent that named its session exits by itself, its rollout is found by the
session in `sessions/YYYY/MM/DD`, newest day first, never through a link, and
read from its end, 64 KiB at first and at most 4 MiB, off the runtime and for
at most 5 seconds. When it reports no window, the window Codex publishes for
the run's model in `models_cache.json` is taken, cut to its effective share,
with no fill. A stream yields it last, after the event that ended the turn;
`execute` and `complete` fold it in with the rest. A run that timed out, was
stopped, was dropped or tripped the tripwire reads no rollout.

## Images

A conversation whose messages carry images (`Message::images`, data URLs or
web addresses) hands them to the agent with the text. `transcript::parts`
renders such a conversation as `transcript::render` does, with each message's
images after the message they came with, a `Part::Image` marked current when it
is on the last user message; `render` and `render_tail` still return the text
alone, unchanged. Claude reads the turn as one stream-json user message
(`--input-format stream-json`) whose content blocks carry each image as base64
or a url; Codex reads the text on stdin, each image marked where it was sent as
`[Image 1]` and on, and opens the images from temporary files named by
`--image=<path>`, which are removed when the run ends. A conversation without
any image is still the rendering alone, written to stdin byte for byte, so
`execute_prompt` and `stream_prompt` keep delivering a slash command as one.

`Image::parse` reads a base64 data URL of a PNG, JPEG, GIF or WebP image, its
media type in any case and its base64 wrapped or unpadded, and checks the bytes
begin as an image of one of those types does, sending them as the type they
are; an `http` or `https` address is a link. An image on the message being
answered that cannot be sent fails the run before the agent starts, as an
`Undeliverable` naming the agent and the `Rejection`. One on an earlier message
is noted in its place instead, so a conversation whose history holds such an
image keeps working: a link (which may have died, and Codex cannot open), data
that does not decode to the image it claims, one over the 10 MB of base64 an
image may be, and, keeping the newest first, any past the 100 images or the
request bytes `Limits::CLAUDE` allows one Claude request, its text sized as the
JSON it is sent as. Once one image is crowded out, every older one is too.

```rust
use abnegate_agent_cli::Image;
use abnegate_agent_cli::Part;
use abnegate_agent_cli::transcript;
use abnegate_llm::Message;

const PNG: &str = "data:image/png;base64,iVBORw0KGgo=";

let mut message = Message::user("What does this show?");
message.images = vec![PNG.to_string()];
let conversation = [message];

assert_eq!(transcript::render(&conversation), "User:\nWhat does this show?");
assert_eq!(
    transcript::parts(&conversation),
    [Part::text("User:\nWhat does this show?"), Part::image(PNG, true)]
);
assert!(matches!(
    Image::parse(PNG),
    Ok(Image::Inline { media_type: "image/png", .. })
));
```

## Sessions

Claude keeps a session at `projects/<project>/<id>.jsonl` in its configuration
directory, and Codex at `sessions/<yyyy>/<mm>/<dd>/rollout-<stamp>-<id>.jsonl`
in its `CODEX_HOME`. `Session::locate` finds that file in a home for runs in a
working directory, and `Session::carry` copies the newest copy of it from one
home into another under the path the agent looks for it there, so a run signed
in to the other home resumes the conversation. The copy is staged beside its
destination at `0600`, synced, and renamed into place, so a file already there
is replaced whole; each directory missing on the way is created at `0700`.
`Session::relocate` moves a Claude session to the folder of the directory its
next run starts in and removes the older copies carrying left behind; a Codex
session is found by its id alone and stays where it is. `Session::project` is
the folder name Claude gives a working directory: the path with every link
resolved and each UTF-16 unit that is not an ASCII letter or digit made `-`,
cut at 200 units and followed by a base-36 hash when it is longer. None of them
follows a link between the home and the file, and an id other than ASCII
letters, digits and `-` is refused as `SessionError::Invalid`.

```rust,no_run
use std::path::Path;

use abnegate_agent_cli::AgentKind;
use abnegate_agent_cli::Session;

let session = Session::resumed("6f1b2c3d-4e5f-4a6b-8c7d-9e0f1a2b3c4d");
let carried = session.carry(
    AgentKind::Claude,
    Path::new("/home/agent/.claude-first"),
    Path::new("/home/agent/.claude-second"),
    Path::new("/path/to/repository"),
)?;
println!("{} bytes at {}", carried.bytes, carried.path.display());
# Ok::<(), abnegate_agent_cli::SessionError>(())
```

## Failures

A failure the agent reported is its own words, then `STDERR_HEADING`, then the
last whole lines of its stderr that fit in 1 KiB: some agents give the reason
for a failure, such as a sign-in that could not be renewed, only on stderr.
Split on `STDERR_HEADING` to read the agent's words alone. An agent that exits
nonzero with no report of its own fails with the end of its stderr. Stderr is
read to its end, every line of it checked against the tripwire, and at most
its last `output_limit` bytes are kept in `Execution::stderr`, starting past
any secret or credential-shaped token the cut would split, so no piece of one
is kept unscrubbed. A run stopped for
prose past its output limit fails with a message starting with
`PROSE_EXCEEDED`, so a caller can tell it from any other failure.

Output a process the agent forked still holds open after the agent exits does
not hold the run up. A run that cleans up kills the agent's process group at
once; otherwise that process gets 5 seconds to finish before the group is
killed. Then the run drains the agent's stdout and stderr: everything already
written is read, however far behind the run fell, and the run stops once the
output goes quiet 3 seconds into the drain, or 10 seconds into it while a
process outside the group keeps writing, keeping what it read.

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
