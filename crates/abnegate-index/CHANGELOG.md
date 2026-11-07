# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.1](https://github.com/abnegate/crates/compare/abnegate-index/v0.1.0...abnegate-index/v0.1.1) - 2026-11-07

### Added

- *(index)* add code chunking, query rewriting and hybrid ranking ([#70](https://github.com/abnegate/crates/pull/70))

## [0.1.0] - 2026-10-02

### Added

- Overlapping-window source chunking, tree walk, and host-owned index store
- Host `Store` and `Embedder` traits, with in-memory `Memory` and optional embeddings
- Generated and minified skip, SHA-256 unchanged-file skip, and keyword/cosine/vector ranking
