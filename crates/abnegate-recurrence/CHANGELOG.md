# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0] - 2026-11-07

### Added

- `Recurrence`, which parses an RRULE over `FREQ`, `INTERVAL`, `BYDAY`,
  `BYHOUR`, `BYMINUTE`, `BYMONTHDAY`, `UNTIL` and `COUNT`, reports its
  shortest gap between firings with `period`, and answers `fires_at` and
  `next_after` on a `Timezone`'s wall clock, where a time the clocks skip fires
  at the instant they change and a time they repeat fires at the first of the
  two.
