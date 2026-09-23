# abnegate/crates

Shared Rust infrastructure, extracted from the applications that each grew their
own copy of it. Every crate here started from one named donor implementation and
absorbed the unique capabilities of the others, so a fix lands once and every
consumer gets it. Each crate is edition 2024, MSRV 1.97, caret-ranged, and keeps
its heavy dependencies behind features.

## Crates

| Crate | Description | Features | Docs |
|---|---|---|---|
| `abnegate-secret` | Secret values that zeroize on drop, redact in Debug and logs, and encrypt at rest with an AES-256-GCM envelope | `rusqlite`, `sqlx` | [![docs.rs](https://img.shields.io/docsrs/abnegate-secret)](https://docs.rs/abnegate-secret) |
| `abnegate-http` | An HTTP client abstraction, SSRF-guarded outbound fetches, keyed rate limiting, exponential backoff and failure classification | none | [![docs.rs](https://img.shields.io/docsrs/abnegate-http)](https://docs.rs/abnegate-http) |
| `abnegate-config` | Typed TOML configuration for command line applications: a generic file loader that transparently decrypts sealed values, an OS keyring token store, and idempotent .env updates | `keyring` | [![docs.rs](https://img.shields.io/docsrs/abnegate-config)](https://docs.rs/abnegate-config) |
| `abnegate-exec` | Sandboxed command execution with streaming output: seatbelt on macOS, bubblewrap on Linux, process-group cancellation and an NDJSON job protocol | none | [![docs.rs](https://img.shields.io/docsrs/abnegate-exec)](https://docs.rs/abnegate-exec) |
| `abnegate-llm` | OpenAI-compatible chat completions with SSE streaming and weighted provider routing, modality-axis provider traits, cost estimation, local hardware profiling and model catalogue browsing | `anthropic`, `catalog`, `download`, `google`, `openai` | [![docs.rs](https://img.shields.io/docsrs/abnegate-llm)](https://docs.rs/abnegate-llm) |
| `abnegate-agent-cli` | Coding agent CLIs (Claude Code, Codex) driven as child processes behind the abnegate-llm completion provider contract, with bounded NDJSON framing, stream parsers, MCP attachment and execution logs | none | [![docs.rs](https://img.shields.io/docsrs/abnegate-agent-cli)](https://docs.rs/abnegate-agent-cli) |
| `abnegate-agent` | A ReAct agent loop with a sandbox-aware tool registry, background jobs, token-aware context compaction, chat history storage, session persistence, prompt templates and an optional MCP client | `mcp` | [![docs.rs](https://img.shields.io/docsrs/abnegate-agent)](https://docs.rs/abnegate-agent) |
| `abnegate-notify` | One notification delivered to every channel at once, with per-channel isolation and SSRF-guarded webhooks | `smtp` | [![docs.rs](https://img.shields.io/docsrs/abnegate-notify)](https://docs.rs/abnegate-notify) |
| `abnegate-vcs` | Git operations over the git command line: branches, worktrees, merge-conflict reproduction, dependency discovery and GitHub pull requests | `github` | [![docs.rs](https://img.shields.io/docsrs/abnegate-vcs)](https://docs.rs/abnegate-vcs) |
| `abnegate-search` | SearXNG metasearch client with a web-search intent heuristic and prompt-ready result formatting | none | [![docs.rs](https://img.shields.io/docsrs/abnegate-search)](https://docs.rs/abnegate-search) |
| `abnegate-vision` | Subject-aware image cropping: decode JPEG, PNG and WebP, locate the salient subject with U2-Net on ONNX Runtime, and render the crop in a single resampling pass | `saliency` | [![docs.rs](https://img.shields.io/docsrs/abnegate-vision)](https://docs.rs/abnegate-vision) |
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
