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
conversation, whose role labels are escaped. A run can start the agent's own
`Session` under an id of the caller's choosing, or resume one. A Claude or Codex
run can attach MCP servers, and a Claude run can restrict its tools, or be
confined to reading its working directory, and a run
that fails in any way takes every process the agent forked with it. The agent is
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
but what the settings allow. Claude's own tools keep Claude's own permission
checks. Codex's do not: `codex exec` never asks for approval, so only its
sandbox confines them, whichever one `CliSettings::with_sandbox`, the user's
configuration or a `--sandbox` argument names, and `danger-full-access` confines
nothing.

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
status is 401. A subagent's lines contribute only the tools it calls: its
words, token counts and refusals stay in its own conversation, and the main
agent answers without them.

A successful result's text is the turn's final answer, `AgentEvent::Answer`,
yielded just before `AgentEvent::Finished` and kept in
`StdoutParseResult::answer` apart from the narration streamed as
`AgentEvent::Text` on the way there. `StdoutParseResult::reply` is that answer,
or all the prose when the agent gave none, and is what `complete` returns. A
failed result, such as `error_max_turns`, fails in its words and then the
`errors` it lists, a line apiece, and in its subtype only when it gave neither.

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

## Login

With the `login` feature, `login::claude` signs Claude Code in the way
`claude setup-token` does. `Authorization::new` draws a PKCE verifier and
state and builds the link to open; claude.com shows the code to paste, read
with `Code::parse`, or sends the browser to a loopback listener at
`http://localhost:<port>/callback`, whose query `Reply::read` reads. A
`Client` exchanges the code for `Tokens` and renews them. `Endpoints` holds
the OAuth client and its URLs, the claude CLI's own by default; each URL is
checked as it is set, refusing link-local and cloud metadata targets but not
loopback or LAN hosts, and the redirect page carries no query or fragment.
The client follows no redirect, names itself `abnegate-agent-cli/<version>`,
and goes through no proxy unless one is given with `Client::with_proxy`,
checked as the endpoints are; it never reads one from the environment. Build
it once and clone it: clones share its connection pool.

`login::codex` signs Codex in the way `codex login --device-auth` does.
`DeviceLogin::start` runs codex with exactly the environment it is given, in
a process group of its own with nothing on its stdin, its `CODEX_HOME` a
private (0700) staging directory under `<root>/.login`: codex deletes any
login in its home before it prints a code, so no existing login is touched
until the new one is promoted. `root` must be writable by this user alone.
It returns once codex has printed the `Prompt`, the link and one-time code a
person enters, whose `Debug` hides the code; dropping the `start` future
before then stops codex. The `Device` it hands back is opaque: `prompt`
shows the prompt, `outcome` resolves to the `Staging` directory once codex
exits having saved a regular-file `auth.json` there, and gives nothing up
when a wait on it is abandoned, such as by a timeout. `cancel`, dropping the
`Device`, and the code's expiry each stop codex, terminating its whole group
and killing what is left of it after a grace period, and remove the staging
directory, unless codex had already saved a login there: a stopped codex that
exits cleanly leaving an `auth.json` that names an account resolves `outcome`
to its `Staging` all the same. `Staging::promote` moves the login into a home
in one rename, `codex::signed_in` says whether a home holds one, and
`Account` reads whose ChatGPT login it is, its `key` telling one login from
another. `DeviceLogin::logout` runs `codex logout` against a home, and
`Limits` bounds how long each stage may take.

`login::Status::check` asks either CLI whether it is signed in, labelled with
its plan or how it signed in, and `Status::check_within` does so within a
limit of the caller's. Every CLI is described by a `login::Cli`: its
executable, the variables it is given, and its secrets, such as a proxy URL
with a password, which are scrubbed from every reason a step fails with.
`DeviceLogin` takes the same variables and secrets.

Nothing is stored for you: `Tokens` serialise their secrets in plaintext, so
seal them before they reach storage, and a staged codex login is codex's own
`auth.json`, to promote or read and keep however you keep credentials. A
`LoginError` from the HTTP client never repeats a response body or anything
the request sent: a refusal is its HTTP status and the `Refusal` its OAuth
error names. A CLI's failure is passed on in its own words, which may quote a
server, after its one-time code, its secrets and anything shaped like a
credential are redacted.

## Features

- `login`: the `login` module, which signs an agent's CLI in: `login::claude`
  runs Claude's PKCE sign-in and hands back its `Tokens`, `login::codex` runs
  Codex's device-code sign-in and hands back the login it saved, and
  `login::Status` asks either CLI whether it is signed in.

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
