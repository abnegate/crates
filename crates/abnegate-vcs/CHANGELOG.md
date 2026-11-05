# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.8](https://github.com/abnegate/crates/compare/abnegate-vcs/v0.1.7...abnegate-vcs/v0.1.8) - 2026-11-05

### Added

- *(vcs)* read a configuration file's origin without trusting the file or the environment ([#42](https://github.com/abnegate/crates/pull/42))

### Fixed

- *(vcs)* refuse a configured_origin path that is not a regular file ([#45](https://github.com/abnegate/crates/pull/45))

## [0.1.7](https://github.com/abnegate/crates/compare/abnegate-vcs/v0.1.6...abnegate-vcs/v0.1.7) - 2026-11-05

### Added

- *(vcs)* set pull requests to merge once ready and move them in and out of draft

### Fixed

- *(vcs)* read an unstable or moved pull request's auto-merge refusal as what it is
- *(vcs)* trust a change only when github's answer says it made it

## [0.1.6](https://github.com/abnegate/crates/compare/abnegate-vcs/v0.1.5...abnegate-vcs/v0.1.6) - 2026-11-03

### Added

- *(vcs)* so zone and claudear can drop their own git and review plumbing ([#19](https://github.com/abnegate/crates/pull/19))

## [0.1.5](https://github.com/abnegate/crates/compare/abnegate-vcs/v0.1.4...abnegate-vcs/v0.1.5) - 2026-11-03

### Added

- *(vcs)* register, re-sign and remove repository webhooks ([#17](https://github.com/abnegate/crates/pull/17))

### Other

- *(release)* nest git tags as crate/vversion ([#15](https://github.com/abnegate/crates/pull/15))

## [0.1.4](https://github.com/abnegate/crates/compare/abnegate-vcs/v0.1.3...abnegate-vcs/v0.1.4) - 2026-10-09

### Other

- updated the following local packages: abnegate-secret

## [0.1.3](https://github.com/abnegate/crates/compare/abnegate-vcs/v0.1.2...abnegate-vcs/v0.1.3) - 2026-10-01

### Other

- updated the following local packages: abnegate-secret

## [0.1.2](https://github.com/abnegate/crates/compare/abnegate-vcs/v0.1.1...abnegate-vcs/v0.1.2) - 2026-09-30

### Added

- *(vcs)* refresh a branch by merging its base in and pushing the merge

### Fixed

- *(vcs)* report a refresh merge that fails without a conflict as a failure

### Other

- describe four items as the merged port left them
- *(vcs)* document every public item
- *(vcs)* pin that a push moves no branch a fetch refspec names

## [0.1.1](https://github.com/abnegate/crates/compare/abnegate-vcs/v0.1.0...abnegate-vcs/v0.1.1) - 2026-09-30

### Other

- Merge branch 'main' into release/trusted-publishing
- *(release)* tag, release and version each crate on its own
