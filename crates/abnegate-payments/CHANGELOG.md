# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.1](https://github.com/abnegate/crates/compare/abnegate-payments-v0.1.0...abnegate-payments-v0.1.1) - 2026-10-09

### Other

- updated the following local packages: abnegate-secret

## [0.1.0] - 2026-10-09

### Added

- Stripe Checkout, Customer Portal and signed webhook events behind `Client`
  and `Payments`, with a `testing` fake that never talks to Stripe.
