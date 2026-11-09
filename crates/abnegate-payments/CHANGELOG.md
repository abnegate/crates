# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.1](https://github.com/abnegate/crates/compare/abnegate-payments/v0.2.0...abnegate-payments/v0.2.1) - 2026-11-09

### Other

- release ([#79](https://github.com/abnegate/crates/pull/79))

## [0.2.0](https://github.com/abnegate/crates/compare/abnegate-payments/v0.1.4...abnegate-payments/v0.2.0) - 2026-11-09

### Fixed

- *(deps)* [**breaking**] move rmcp past its advisories and drop the rusqlite feature ([#78](https://github.com/abnegate/crates/pull/78))

## [0.1.4](https://github.com/abnegate/crates/compare/abnegate-payments/v0.1.3...abnegate-payments/v0.1.4) - 2026-11-06

### Other

- updated the following local packages: abnegate-secret

## [0.1.3](https://github.com/abnegate/crates/compare/abnegate-payments/v0.1.2...abnegate-payments/v0.1.3) - 2026-11-05

### Other

- updated the following local packages: abnegate-secret

## [0.1.2](https://github.com/abnegate/crates/compare/abnegate-payments/v0.1.1...abnegate-payments/v0.1.2) - 2026-11-03

### Other

- *(release)* nest git tags as crate/vversion ([#15](https://github.com/abnegate/crates/pull/15))

## [0.1.1](https://github.com/abnegate/crates/compare/abnegate-payments/v0.1.0...abnegate-payments/v0.1.1) - 2026-10-09

### Other

- updated the following local packages: abnegate-secret

## [0.1.0] - 2026-10-09

### Added

- Stripe Checkout, Customer Portal and signed webhook events behind `Client`
  and `Payments`, with a `testing` fake that never talks to Stripe.
