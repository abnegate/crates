# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.14](https://github.com/abnegate/crates/compare/abnegate-agent-cli/v0.1.13...abnegate-agent-cli/v0.1.14) - 2026-11-07

### Added

- *(agent-cli)* let a run wait for claude's background work ([#74](https://github.com/abnegate/crates/pull/74))
- *(agent-cli)* sign claude's token client in to a proxy that wants it ([#75](https://github.com/abnegate/crates/pull/75))

### Fixed

- *(agent-cli)* show a subagent's report without claude's frame ([#72](https://github.com/abnegate/crates/pull/72))

## [0.1.13](https://github.com/abnegate/crates/compare/abnegate-agent-cli/v0.1.12...abnegate-agent-cli/v0.1.13) - 2026-11-06

### Added

- *(abnegate-agent-cli)* bring the cli provider up to the host's own ([#60](https://github.com/abnegate/crates/pull/60))
- *(abnegate-agent-cli)* sign claude code and codex in ([#61](https://github.com/abnegate/crates/pull/61))

## [0.1.12](https://github.com/abnegate/crates/compare/abnegate-agent-cli/v0.1.11...abnegate-agent-cli/v0.1.12) - 2026-11-06

### Other

- updated the following local packages: abnegate-secret, abnegate-llm, abnegate-exec

## [0.1.11](https://github.com/abnegate/crates/compare/abnegate-agent-cli/v0.1.10...abnegate-agent-cli/v0.1.11) - 2026-11-05

### Other

- updated the following local packages: abnegate-secret, abnegate-llm, abnegate-exec

## [0.1.10](https://github.com/abnegate/crates/compare/abnegate-agent-cli/v0.1.9...abnegate-agent-cli/v0.1.10) - 2026-11-03

### Other

- updated the following local packages: abnegate-exec

## [0.1.9](https://github.com/abnegate/crates/compare/abnegate-agent-cli/v0.1.8...abnegate-agent-cli/v0.1.9) - 2026-11-03

### Fixed

- *(llm)* so an agent timeout reads "1800 seconds", not "1800s"
- *(llm)* so a window equals itself after a trip through JSON

## [0.1.8](https://github.com/abnegate/crates/compare/abnegate-agent-cli/v0.1.7...abnegate-agent-cli/v0.1.8) - 2026-11-03

### Other

- updated the following local packages: abnegate-exec

## [0.1.7](https://github.com/abnegate/crates/compare/abnegate-agent-cli/v0.1.6...abnegate-agent-cli/v0.1.7) - 2026-11-03

### Added

- *(agent-cli)* so a run answers in its final result and fails in the errors claude gave
- *(agent-cli)* so output an agent wrote before it exited is read even while an escaped process holds the pipe open
- *(agent-cli)* so a run's cleanup never signals a group it no longer owns and can reach what left the group
- *(agent-cli)* so a slash command reaches the agent as the caller typed it
- *(agent-cli)* so a host can tell a turn a usage limit refused from a broken one
- *(agent-cli)* so a host can stream a turn and keep its own sign-in from the agents it runs
- *(agent-cli)* so a host can pin, resume and sandbox an agent's own session

## [0.1.6](https://github.com/abnegate/crates/compare/abnegate-agent-cli/v0.1.5...abnegate-agent-cli/v0.1.6) - 2026-11-03

### Other

- updated the following local packages: abnegate-exec, abnegate-llm

## [0.1.5](https://github.com/abnegate/crates/compare/abnegate-agent-cli/v0.1.4...abnegate-agent-cli/v0.1.5) - 2026-11-03

### Other

- *(release)* nest git tags as crate/vversion ([#15](https://github.com/abnegate/crates/pull/15))

## [0.1.4](https://github.com/abnegate/crates/compare/abnegate-agent-cli/v0.1.3...abnegate-agent-cli/v0.1.4) - 2026-10-09

### Other

- updated the following local packages: abnegate-secret, abnegate-exec, abnegate-llm

## [0.1.3](https://github.com/abnegate/crates/compare/abnegate-agent-cli/v0.1.2...abnegate-agent-cli/v0.1.3) - 2026-10-01

### Other

- updated the following local packages: abnegate-secret, abnegate-llm, abnegate-exec

## [0.1.2](https://github.com/abnegate/crates/compare/abnegate-agent-cli/v0.1.1...abnegate-agent-cli/v0.1.2) - 2026-09-30

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

## [0.1.1](https://github.com/abnegate/crates/compare/abnegate-agent-cli/v0.1.0...abnegate-agent-cli/v0.1.1) - 2026-09-30

### Added

- *(agent-cli)* attach MCP servers to Codex the way Codex takes them

### Fixed

- *(agent-cli)* keep a URL that could carry a credential off Codex's command line
- *(agent-cli)* give Codex each tools entry as written

### Other

- Merge branch 'main' into release/trusted-publishing
- merge the notify, agent search and MCP naming, and Codex MCP 0.1.1 changes
