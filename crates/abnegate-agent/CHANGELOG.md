# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.4](https://github.com/abnegate/crates/compare/abnegate-agent-v0.1.3...abnegate-agent-v0.1.4) - 2026-10-09

### Other

- updated the following local packages: abnegate-secret, abnegate-exec, abnegate-llm, abnegate-agent-cli, abnegate-config

## [0.1.3](https://github.com/abnegate/crates/compare/abnegate-agent-v0.1.2...abnegate-agent-v0.1.3) - 2026-10-01

### Other

- updated the following local packages: abnegate-secret, abnegate-llm, abnegate-exec, abnegate-agent-cli, abnegate-config

## [0.1.2](https://github.com/abnegate/crates/compare/abnegate-agent-v0.1.1...abnegate-agent-v0.1.2) - 2026-09-30

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

## [0.1.1](https://github.com/abnegate/crates/compare/abnegate-agent-v0.1.0...abnegate-agent-v0.1.1) - 2026-09-30

### Other

- Merge branch 'main' into release/trusted-publishing
- *(release)* tag, release and version each crate on its own
