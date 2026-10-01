# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Digest stamps each recent trial as `learn_tried_N` with its summary, lesson, and tags

## [0.1.0] - 2026-10-01

### Added

- In-memory trial memory, similar-outcome retrieval, and avoid/context/instruction suggestions
- Host `Archive` and `Embedder` traits, with `Memory` restore/persist/record_embedded/embed_missing
- `ErrorClass` keyword classification and embeddable `REFERENCES`
- `Fingerprint` log parse (host-supplied action names) and `Extractor` lesson parse
- `tags_from` keyword helper, cosine/euclidean/normalize, and serde on public types
- `Extractor` decision phrases (`decided to`, `chose to`, `went with`)
