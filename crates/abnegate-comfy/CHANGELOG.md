# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- `Client::generate_excluding` takes a negative prompt, so what a picture must not show stays out of its prompt; `generate` is unchanged
- `recipe::Fill::with_negative` and the catalog's `slots.negative`, which every packaged image recipe now declares

## [0.1.8](https://github.com/abnegate/crates/compare/abnegate-comfy/v0.1.7...abnegate-comfy/v0.1.8) - 2026-11-03

### Other

- updated the following local packages: abnegate-exec

## [0.1.7](https://github.com/abnegate/crates/compare/abnegate-comfy/v0.1.6...abnegate-comfy/v0.1.7) - 2026-11-03

### Other

- updated the following local packages: abnegate-exec, abnegate-llm

## [0.1.6](https://github.com/abnegate/crates/compare/abnegate-comfy/v0.1.5...abnegate-comfy/v0.1.6) - 2026-11-03

### Other

- updated the following local packages: abnegate-exec, abnegate-llm

## [0.1.5](https://github.com/abnegate/crates/compare/abnegate-comfy/v0.1.4...abnegate-comfy/v0.1.5) - 2026-11-03

### Other

- *(release)* nest git tags as crate/vversion ([#15](https://github.com/abnegate/crates/pull/15))

## [0.1.4](https://github.com/abnegate/crates/compare/abnegate-comfy/v0.1.3...abnegate-comfy/v0.1.4) - 2026-10-09

### Other

- updated the following local packages: abnegate-secret, abnegate-exec, abnegate-llm

## [0.1.3](https://github.com/abnegate/crates/compare/abnegate-comfy/v0.1.2...abnegate-comfy/v0.1.3) - 2026-10-01

### Other

- updated the following local packages: abnegate-secret, abnegate-llm, abnegate-exec

## [0.1.2](https://github.com/abnegate/crates/compare/abnegate-comfy/v0.1.1...abnegate-comfy/v0.1.2) - 2026-09-30

### Fixed

- *(comfy)* refuse a blank or control-character decoder name
- *(comfy)* join only single path segments onto the models directory

### Other

- describe four items as the merged port left them
- *(comfy)* document every public item

## [0.1.1](https://github.com/abnegate/crates/compare/abnegate-comfy/v0.1.0...abnegate-comfy/v0.1.1) - 2026-09-30

### Other

- Merge branch 'main' into release/trusted-publishing
- *(release)* tag, release and version each crate on its own
