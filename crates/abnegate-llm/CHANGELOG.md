# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.8](https://github.com/abnegate/crates/compare/abnegate-llm/v0.1.7...abnegate-llm/v0.1.8) - 2026-11-03

### Added

- *(llm)* so a host can offer xhigh, max and ultra reasoning without a copy of its own

### Fixed

- *(llm)* so an agent timeout reads "1800 seconds", not "1800s"
- *(llm)* so a window equals itself after a trip through JSON

## [0.1.7](https://github.com/abnegate/crates/compare/abnegate-llm/v0.1.6...abnegate-llm/v0.1.7) - 2026-11-03

### Added

- *(llm)* so a tenant's endpoint cannot reach metadata or leak the key it was sent
- *(llm)* so a host can keep every model exchange as an ndjson log
- *(llm)* so a request sent straight to openai or anthropic is not refused with a 400
- *(llm)* so a usage limit reaches the caller as a limit, not as agent prose

### Fixed

- *(llm)* so the claude-cli route names itself, retries what the api retries, and a cost strategy typo is reported

## [0.1.6](https://github.com/abnegate/crates/compare/abnegate-llm/v0.1.5...abnegate-llm/v0.1.6) - 2026-11-03

### Added

- *(llm)* so a host can tell a missing repository apart and show a model's quantization

### Fixed

- *(llm)* so loaders sharing a cold model cache wait for each other's download

## [0.1.5](https://github.com/abnegate/crates/compare/abnegate-llm/v0.1.4...abnegate-llm/v0.1.5) - 2026-11-03

### Other

- *(release)* nest git tags as crate/vversion ([#15](https://github.com/abnegate/crates/pull/15))

## [0.1.4](https://github.com/abnegate/crates/compare/abnegate-llm/v0.1.3...abnegate-llm/v0.1.4) - 2026-10-09

### Other

- updated the following local packages: abnegate-secret

## [0.1.3](https://github.com/abnegate/crates/compare/abnegate-llm/v0.1.2...abnegate-llm/v0.1.3) - 2026-10-01

### Added

- *(llm)* load local ONNX embeddings through fastembed

### Added

- *(llm)* local ONNX embeddings through fastembed behind the `fastembed` feature

## [0.1.2](https://github.com/abnegate/crates/compare/abnegate-llm/v0.1.1...abnegate-llm/v0.1.2) - 2026-09-30

### Fixed

- *(llm)* refuse a catalogue url that is not http or https

### Other

- stop three docs promising more than the code does
- describe four items as the merged port left them
- *(llm)* document every public item
- *(llm)* send the url the outbound guard checked

## [0.1.1](https://github.com/abnegate/crates/compare/abnegate-llm/v0.1.0...abnegate-llm/v0.1.1) - 2026-09-30

### Other

- *(release)* tag, release and version each crate on its own
