# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.5](https://github.com/abnegate/crates/compare/abnegate-agent/v0.2.4...abnegate-agent/v0.2.5) - 2026-11-13

### Added

- *(exec)* start tool commands and jobs at a configured niceness ([#92](https://github.com/abnegate/crates/pull/92))

## [0.2.4](https://github.com/abnegate/crates/compare/abnegate-agent/v0.2.3...abnegate-agent/v0.2.4) - 2026-11-13

### Other

- updated the following local packages: abnegate-exec, abnegate-agent-cli

## [0.2.3](https://github.com/abnegate/crates/compare/abnegate-agent/v0.2.2...abnegate-agent/v0.2.3) - 2026-11-12

### Other

- updated the following local packages: abnegate-exec, abnegate-agent-cli

## [0.2.2](https://github.com/abnegate/crates/compare/abnegate-agent/v0.2.1...abnegate-agent/v0.2.2) - 2026-11-11

### Added

- *(agent)* report why an mcp server failed to connect as data ([#84](https://github.com/abnegate/crates/pull/84))

## [0.2.1](https://github.com/abnegate/crates/compare/abnegate-agent/v0.2.0...abnegate-agent/v0.2.1) - 2026-11-10

### Added

- *(agent)* connect streamable-http mcp servers and report each server's outcome ([#81](https://github.com/abnegate/crates/pull/81))

## [0.2.0](https://github.com/abnegate/crates/compare/abnegate-agent/v0.1.15...abnegate-agent/v0.2.0) - 2026-11-09

### Fixed

- *(deps)* [**breaking**] move rmcp past its advisories and drop the rusqlite feature ([#78](https://github.com/abnegate/crates/pull/78))

## [0.1.15](https://github.com/abnegate/crates/compare/abnegate-agent/v0.1.14...abnegate-agent/v0.1.15) - 2026-11-07

### Other

- updated the following local packages: abnegate-llm, abnegate-agent-cli

## [0.1.14](https://github.com/abnegate/crates/compare/abnegate-agent/v0.1.13...abnegate-agent/v0.1.14) - 2026-11-06

### Added

- *(abnegate-agent)* bring the agent tools and chat level with the host ([#63](https://github.com/abnegate/crates/pull/63))

## [0.1.13](https://github.com/abnegate/crates/compare/abnegate-agent/v0.1.12...abnegate-agent/v0.1.13) - 2026-11-06

### Added

- *(abnegate-agent-cli)* bring the cli provider up to the host's own ([#60](https://github.com/abnegate/crates/pull/60))

## [0.1.12](https://github.com/abnegate/crates/compare/abnegate-agent/v0.1.11...abnegate-agent/v0.1.12) - 2026-11-06

### Other

- updated the following local packages: abnegate-secret, abnegate-llm, abnegate-exec, abnegate-agent-cli, abnegate-config

## [0.1.11](https://github.com/abnegate/crates/compare/abnegate-agent/v0.1.10...abnegate-agent/v0.1.11) - 2026-11-05

### Other

- updated the following local packages: abnegate-secret, abnegate-llm, abnegate-exec, abnegate-agent-cli, abnegate-config

## [0.1.10](https://github.com/abnegate/crates/compare/abnegate-agent/v0.1.9...abnegate-agent/v0.1.10) - 2026-11-03

### Other

- updated the following local packages: abnegate-exec, abnegate-agent-cli

## [0.1.9](https://github.com/abnegate/crates/compare/abnegate-agent/v0.1.8...abnegate-agent/v0.1.9) - 2026-11-03

### Other

- updated the following local packages: abnegate-llm, abnegate-agent-cli

## [0.1.8](https://github.com/abnegate/crates/compare/abnegate-agent/v0.1.7...abnegate-agent/v0.1.8) - 2026-11-03

### Other

- updated the following local packages: abnegate-exec, abnegate-agent-cli

## [0.1.7](https://github.com/abnegate/crates/compare/abnegate-agent/v0.1.6...abnegate-agent/v0.1.7) - 2026-11-03

### Added

- *(agent)* so a host can route its tools, jobs and mcp servers through a proxy of its own
- *(agent)* so a chat can see images, choose its window and run against endpoints that publish nothing
- *(agent)* so a host can gate the public web, pick its run_command programs and prune its registry

## [0.1.6](https://github.com/abnegate/crates/compare/abnegate-agent/v0.1.5...abnegate-agent/v0.1.6) - 2026-11-03

### Other

- updated the following local packages: abnegate-exec, abnegate-llm, abnegate-config, abnegate-agent-cli

## [0.1.5](https://github.com/abnegate/crates/compare/abnegate-agent/v0.1.4...abnegate-agent/v0.1.5) - 2026-11-03

### Other

- *(release)* nest git tags as crate/vversion ([#15](https://github.com/abnegate/crates/pull/15))

## [0.1.4](https://github.com/abnegate/crates/compare/abnegate-agent/v0.1.3...abnegate-agent/v0.1.4) - 2026-10-09

### Other

- updated the following local packages: abnegate-secret, abnegate-exec, abnegate-llm, abnegate-agent-cli, abnegate-config

## [0.1.3](https://github.com/abnegate/crates/compare/abnegate-agent/v0.1.2...abnegate-agent/v0.1.3) - 2026-10-01

### Other

- updated the following local packages: abnegate-secret, abnegate-llm, abnegate-exec, abnegate-agent-cli, abnegate-config

## [0.1.2](https://github.com/abnegate/crates/compare/abnegate-agent/v0.1.1...abnegate-agent/v0.1.2) - 2026-09-30

### Added

- *(agent)* withhold denied paths from the file tools by identity

### Fixed

- *(agent)* keep denied entries out of a listing by name as well
- *(agent)* refuse a path that runs through more links than the kernel follows
- *(agent)* refuse a job exclude that is not a regular file
- *(agent)* hedge wait_for only for a turn that is not offered it
- *(agent)* judge each search match by its file and bound a listing by the context

### Other

- *(agent)* document every public item

## [0.1.1](https://github.com/abnegate/crates/compare/abnegate-agent/v0.1.0...abnegate-agent/v0.1.1) - 2026-09-30

### Other

- Merge branch 'main' into release/trusted-publishing
- *(release)* tag, release and version each crate on its own
