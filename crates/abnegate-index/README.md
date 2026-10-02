# abnegate-index

Overlapping-window source chunking and host-owned semantic code search. The
crate walks a tree, splits files on line-aware windows, and writes chunks
through a [`Store`] the host implements. Embeddings are optional: keyword
search still works when the host passes no [`Embedder`].

Persistence and models stay in the application. Implement [`Store`] against
the host database and [`Embedder`] against the host model. This crate is the
mechanism. [`Memory`] is an in-memory store for hosts that do not want a
database yet, and for tests.

Application types stay in the host. Tree-sitter AST chunking, SQLite FTS, and
HNSW/vectorlite ranking stay in the application that owns them.

## Features

- `testing`: `Scripted`, an [`Embedder`] that returns inserted vectors so a
  caller can rank without loading a model.

## Usage

```sh
cargo add abnegate-index
```

```rust
use abnegate_index::{Hit, Memory, Method, Store};

let store = Memory::new();
store
    .replace_file(
        "/src",
        "api.rb",
        "rb",
        "abc",
        &[("get ':id/entities'".into(), Some(vec![1.0, 0.0]))],
    )
    .unwrap();

let hits: Vec<Hit> = store.search_vector(&[1.0, 0.0], Some("/src"), 4).unwrap();
assert_eq!(hits[0].relative, "api.rb");
assert_eq!(hits[0].method, Method::Cosine);
```

Index a tree with [`index_tree`]. Pass [`Proceed`] when the host has no
cancellation, or implement [`Cancel`]. When `embedder` is `None`, text still
lands in the store so later keyword search works. A second pass over an
unchanged file is skipped by content hash.

## Moving from an in-house code index

- Chunking is overlapping windows split on line boundaries. AST-aware
  splitting stays in the host.
- The store is a host trait. This crate never opens a database.
- Embeddings are supplied per chunk, or through an [`Embedder`] the host
  implements. This crate does not load a model.
