# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.0](https://github.com/abnegate/crates/compare/abnegate-secret/v0.1.7...abnegate-secret/v0.2.0) - 2026-11-09

### Fixed

- *(deps)* [**breaking**] move rmcp past its advisories and drop the rusqlite feature ([#78](https://github.com/abnegate/crates/pull/78))

## [0.1.7](https://github.com/abnegate/crates/compare/abnegate-secret/v0.1.6...abnegate-secret/v0.1.7) - 2026-11-06

### Added

- *(secret)* derive master keys from passphrases and read pre-envelope values ([#49](https://github.com/abnegate/crates/pull/49))

## [0.1.6](https://github.com/abnegate/crates/compare/abnegate-secret/v0.1.5...abnegate-secret/v0.1.6) - 2026-11-05

### Added

- *(secret)* tell whether a value meant to be a plain name carries a credential ([#41](https://github.com/abnegate/crates/pull/41))

### Fixed

- *(secret)* stop contains_credential refusing key-word-namespaced names and missing folded prefixes ([#46](https://github.com/abnegate/crates/pull/46))

### Other

- *(secret)* read the plaintext report in a process no other test shares ([#47](https://github.com/abnegate/crates/pull/47))

## [0.1.5](https://github.com/abnegate/crates/compare/abnegate-secret/v0.1.4...abnegate-secret/v0.1.5) - 2026-11-03

### Other

- *(release)* nest git tags as crate/vversion ([#15](https://github.com/abnegate/crates/pull/15))

## [0.1.4](https://github.com/abnegate/crates/compare/abnegate-secret/v0.1.3...abnegate-secret/v0.1.4) - 2026-10-09

### Added

- *(secret)* so xai and bare sk_ keys are redacted and paths are not
- *(secret)* so a count of tokens is not redacted as a token
- *(secret)* so consumers that only redact skip the cipher dependencies

## [0.1.3](https://github.com/abnegate/crates/compare/abnegate-secret/v0.1.2...abnegate-secret/v0.1.3) - 2026-10-01

### Added

- *(secret)* conceal a known key however a provider echoes it

### Fixed

- *(secret)* match a masked echo with the punctuation a key carries

## [0.1.2](https://github.com/abnegate/crates/compare/abnegate-secret/v0.1.1...abnegate-secret/v0.1.2) - 2026-09-30

### Other

- *(secret)* document every public item

## [0.1.1](https://github.com/abnegate/crates/compare/abnegate-secret/v0.1.0...abnegate-secret/v0.1.1) - 2026-09-30

### Other

- *(release)* tag, release and version each crate on its own
