# abnegate/crates

Shared Rust infrastructure, extracted from the applications that each grew their
own copy of it. Every crate here started from one named donor implementation and
absorbed the unique capabilities of the others, so a fix lands once and every
consumer gets it. Each crate is edition 2024, MSRV 1.97, caret-ranged, and keeps
its heavy dependencies behind features.

## Crates

| Crate | Description | Features | Docs |
|---|---|---|---|
| `abnegate-secret` | Secret values that zeroize on drop, redact in `Debug` and logs, and encrypt at rest with an AES-256-GCM envelope | `sqlx`, `rusqlite` | [![docs.rs](https://img.shields.io/docsrs/abnegate-secret)](https://docs.rs/abnegate-secret) |
| `abnegate-http` | HTTP client trait over reqwest with SSRF validation, rate limiting, and retry backoff | none | [![docs.rs](https://img.shields.io/docsrs/abnegate-http)](https://docs.rs/abnegate-http) |
| `abnegate-config` | TOML configuration files, `.env` upsert, and a keyring-backed token store | `keyring` | [![docs.rs](https://img.shields.io/docsrs/abnegate-config)](https://docs.rs/abnegate-config) |
| `abnegate-exec` | Sandboxed command execution with seatbelt and bubblewrap confinement, streaming output, and an NDJSON job protocol | none | [![docs.rs](https://img.shields.io/docsrs/abnegate-exec)](https://docs.rs/abnegate-exec) |
| `abnegate-llm` | OpenAI-compatible LLM client, modality provider traits, routing and selection, cost estimation, and model catalogue browsing | `openai`, `anthropic`, `google`, `catalog`, `download` | [![docs.rs](https://img.shields.io/docsrs/abnegate-llm)](https://docs.rs/abnegate-llm) |
| `abnegate-agent-cli` | Drivers for coding-agent CLIs, with streaming event parsing, execution logs, and fallback chains | none | [![docs.rs](https://img.shields.io/docsrs/abnegate-agent-cli)](https://docs.rs/abnegate-agent-cli) |
| `abnegate-agent` | Tool trait and registry, MCP hub, ReAct loop, session store, context compaction, and chat history | `mcp` | [![docs.rs](https://img.shields.io/docsrs/abnegate-agent)](https://docs.rs/abnegate-agent) |
| `abnegate-notify` | Multi-channel notification fan-out over Slack, Discord, webhooks, email, Telegram, WhatsApp, SMS, and push | `smtp`, `telegram`, `whatsapp`, `sms`, `push` | [![docs.rs](https://img.shields.io/docsrs/abnegate-notify)](https://docs.rs/abnegate-notify) |
| `abnegate-vcs` | Local git operations, worktrees, pull requests, conflict reproduction, and GitHub/GitLab providers | `github`, `gitlab`, `github-app` | [![docs.rs](https://img.shields.io/docsrs/abnegate-vcs)](https://docs.rs/abnegate-vcs) |
| `abnegate-search` | SearXNG web search client | none | [![docs.rs](https://img.shields.io/docsrs/abnegate-search)](https://docs.rs/abnegate-search) |
| `abnegate-vision` | Image understanding: subject detection and subject-aware cropping for training data | `saliency` | [![docs.rs](https://img.shields.io/docsrs/abnegate-vision)](https://docs.rs/abnegate-vision) |
| `abnegate-comfy` | ComfyUI image, video, and audio generation, model inventory, and LoRA training | `saliency` | [![docs.rs](https://img.shields.io/docsrs/abnegate-comfy)](https://docs.rs/abnegate-comfy) |

## Consuming unreleased changes

A consumer that needs a change before it is released points at a local checkout
from its own `.cargo/config.toml`, so nothing in its `Cargo.toml` changes and
the override never reaches a published manifest:

```toml
[patch.crates-io]
abnegate-secret = { path = "../crates/crates/abnegate-secret" }
abnegate-http = { path = "../crates/crates/abnegate-http" }
```

## License

MIT
