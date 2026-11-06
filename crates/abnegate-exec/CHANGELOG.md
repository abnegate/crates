# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.11](https://github.com/abnegate/crates/compare/abnegate-exec/v0.1.10...abnegate-exec/v0.1.11) - 2026-11-06

### Other

- updated the following local packages: abnegate-secret

## [0.1.10](https://github.com/abnegate/crates/compare/abnegate-exec/v0.1.9...abnegate-exec/v0.1.10) - 2026-11-05

### Other

- updated the following local packages: abnegate-secret

## [0.1.9](https://github.com/abnegate/crates/compare/abnegate-exec/v0.1.8...abnegate-exec/v0.1.9) - 2026-11-03

### Fixed

- *(exec)* so a shutdown never fails on a process group whose members are still exiting

### Other

- *(exec)* so the exiting-member fixture waits for the state its refusal depends on
- *(exec)* so a perl leader never sleeps through its interrupt and the exiting-member test proves the refusal

## [0.1.8](https://github.com/abnegate/crates/compare/abnegate-exec/v0.1.7...abnegate-exec/v0.1.8) - 2026-11-03

### Fixed

- *(exec)* report a forced cancel as cancelled, never as a signalled exit ([#30](https://github.com/abnegate/crates/pull/30))

## [0.1.7](https://github.com/abnegate/crates/compare/abnegate-exec/v0.1.6...abnegate-exec/v0.1.7) - 2026-11-03

### Added

- *(exec)* so a host can pass a ctrl-c on to every run and stop what is left when it shuts down
- *(exec)* so a host can interrupt a run and wait for its leader without reaping it
- *(exec)* so a run can stop the processes that left its process group

## [0.1.6](https://github.com/abnegate/crates/compare/abnegate-exec/v0.1.5...abnegate-exec/v0.1.6) - 2026-11-03

### Added

- *(exec)* so a host can answer pings, choose its proxy and feed stdin through the registry

## [0.1.5](https://github.com/abnegate/crates/compare/abnegate-exec/v0.1.4...abnegate-exec/v0.1.5) - 2026-11-03

### Other

- *(release)* nest git tags as crate/vversion ([#15](https://github.com/abnegate/crates/pull/15))

## [0.1.4](https://github.com/abnegate/crates/compare/abnegate-exec/v0.1.3...abnegate-exec/v0.1.4) - 2026-10-09

### Other

- updated the following local packages: abnegate-secret

## [0.1.3](https://github.com/abnegate/crates/compare/abnegate-exec/v0.1.2...abnegate-exec/v0.1.3) - 2026-10-01

### Other

- updated the following local packages: abnegate-secret

## [0.1.2](https://github.com/abnegate/crates/compare/abnegate-exec/v0.1.1...abnegate-exec/v0.1.2) - 2026-09-30

### Other

- *(exec)* document every public item

## [0.1.1](https://github.com/abnegate/crates/compare/abnegate-exec/v0.1.0...abnegate-exec/v0.1.1) - 2026-09-30

### Other

- Merge branch 'main' into release/trusted-publishing
- *(release)* tag, release and version each crate on its own
