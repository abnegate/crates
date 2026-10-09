# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.4](https://github.com/abnegate/crates/compare/abnegate-agent-cli-v0.1.3...abnegate-agent-cli-v0.1.4) - 2026-10-09

### Other

- updated the following local packages: abnegate-secret, abnegate-exec, abnegate-llm

## [0.1.3](https://github.com/abnegate/crates/compare/abnegate-agent-cli-v0.1.2...abnegate-agent-cli-v0.1.3) - 2026-10-01

### Other

- updated the following local packages: abnegate-secret, abnegate-llm, abnegate-exec

## [0.1.2](https://github.com/abnegate/crates/compare/abnegate-agent-cli-v0.1.1...abnegate-agent-cli-v0.1.2) - 2026-09-30

### Added

- *(agent-cli)* report codex mcp tool calls as tool events

### Fixed

- *(agent-cli)* keep the end of a stderr line that holds no space
- *(agent-cli)* never keep a piece of a secret where stderr's tail is cut
- *(agent-cli)* never pass an allowed credential beside the caller's own
- *(agent-cli)* carry the end of an agent's stderr in the failure it reports
- *(agent-cli)* read a claude turn as a whole, keeping subagents out of it

### Other

- describe four items as the merged port left them
- *(agent-cli)* document every public item

## [0.1.1](https://github.com/abnegate/crates/compare/abnegate-agent-cli-v0.1.0...abnegate-agent-cli-v0.1.1) - 2026-09-30

### Added

- *(agent-cli)* attach MCP servers to Codex the way Codex takes them

### Fixed

- *(agent-cli)* keep a URL that could carry a credential off Codex's command line
- *(agent-cli)* give Codex each tools entry as written

### Other

- Merge branch 'main' into release/trusted-publishing
- merge the notify, agent search and MCP naming, and Codex MCP 0.1.1 changes
