# abnegate-index

Source chunking, query rewriting, learned ranking and hybrid fusion for
host-owned semantic code search. The crate walks a tree, splits files on
line-aware windows or, behind a feature, on tree-sitter symbol boundaries, and
writes chunks through a [`Store`] the host implements. Embeddings are optional:
keyword search still works when the host passes no [`Embedder`].

Persistence and models stay in the application. Implement [`Store`] against
the host database and [`Embedder`] against the host model. This crate is the
mechanism. [`Memory`] is an in-memory store for hosts that do not want a
database yet, and for tests.

Search is split the same way. The host runs the keyword and the vector search
against its own indexes; this crate rewrites the question for the keyword leg,
fuses the two legs by reciprocal rank, and reranks the fused chunks with a
linear ranker whose weights the host supplies.

## Features

- `code-chunking`: tree-sitter code chunking through `chunk_code`,
  `chunk_code_with_config` and `smart_chunk`, for Rust, Python, JavaScript,
  TypeScript, Go, Java, C, C++, Kotlin, Swift, SQL, JSON, YAML, Ruby and PHP.
  It adds one grammar crate per language.
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

## Chunking

[`chunks`] cuts a file into overlapping windows on line boundaries.
[`chunk_text`] cuts prose into [`TextChunk`]s of about a token budget, breaking
at a paragraph, sentence, line or word near the end of each window.

```rust
use abnegate_index::chunk_text;

let text = "First paragraph.\n\nSecond paragraph.";
let chunks = chunk_text(text, 512, 64);
assert_eq!(chunks.len(), 1);
assert_eq!(chunks[0].text, text);
```

With `code-chunking`, `chunk_code` splits source on symbol boundaries, never at
a fixed length. Each symbol gets a body chunk and a skeleton of its signature;
the file gets a header chunk with its imports and an API surface chunk with its
public signatures. Every `CodeChunk` carries the identifiers it defines and
references, so keyword and vector search see the same names. `smart_chunk` uses
the code chunker when the language is known and text windows otherwise.

```rust
# #[cfg(feature = "code-chunking")] {
use abnegate_index::{CodeChunkType, CodeLanguage, chunk_code};

let source = "pub fn greet(name: &str) -> String {\n    format!(\"Hello, {name}\")\n}\n";
let chunks = chunk_code(source, CodeLanguage::Rust, 512);

let body = chunks
    .iter()
    .find(|chunk| chunk.chunk_type == CodeChunkType::Body)
    .unwrap();
assert_eq!(body.metadata.symbol.as_deref(), Some("greet"));
# }
```

## Query rewriting

[`rewrite_query`] turns a question into a [`RewrittenQuery`]: the code
identifiers it names, kept intact, and a keyword query that ORs them with the
question's distinctive words. A full-text index then matches `should_skip_blob`
instead of stemming it into `shouldskipblob`.

```rust
use abnegate_index::rewrite_query;

let rewritten = rewrite_query("Where is should_skip_blob called?");
assert_eq!(rewritten.identifiers, ["should_skip_blob"]);
assert!(rewritten.keyword.contains("should_skip_blob"));
```

## Ranking and fusion

[`reciprocal_rank_fusion`] merges the keyword and semantic legs, each a ranked
list of [`Candidate`]s, by position, weighted by a [`HybridSearchConfig`].
[`finalize_ranking`] reranks the fused results with [`score_hit`], drops
fixtures, and keeps at most two chunks per file. [`score_hit`] blends a
[`LinearRanker`] over [`extract_features`] with [`lexical_cross_score`] and
rewards definitions that cover the question's identifiers. Train the weights
with [`fit_pairwise`] on `(better, worse)` feature pairs; this crate ships no
trained ranker.

```rust
use abnegate_index::{
    Candidate, FEATURE_COUNT, HybridSearchConfig, LinearRanker, finalize_ranking,
    reciprocal_rank_fusion,
};

let query = "Where is verify_signature defined?";
let keyword = vec![
    Candidate::new(1, "fn verify_signature() {}", "src/webhook.rs", "webhook.rs", 4.2),
    Candidate::new(2, "verify_signature();", "src/routes.rs", "routes.rs", 1.3),
];
let semantic = vec![
    Candidate::new(1, "fn verify_signature() {}", "src/webhook.rs", "webhook.rs", 0.82),
    Candidate::new(3, "struct Signature;", "src/signature.rs", "signature.rs", 0.61),
];

let fused = reciprocal_rank_fusion(keyword, semantic, &HybridSearchConfig::for_query(query));
assert_eq!(fused[0].id, 1);
assert_eq!(fused[0].keyword_rank, Some(1));
assert_eq!(fused[0].semantic_rank, Some(1));

let ranker = LinearRanker::new([0.0; FEATURE_COUNT], 0.0);
let ranked = finalize_ranking(&ranker, fused, query, 10);
assert_eq!(ranked[0].item_uri, "src/webhook.rs");
```

## Moving from an in-house code index

- Window chunking needs no feature. Tree-sitter chunking is behind
  `code-chunking`, so a host that does not use it pulls in no grammar.
- The store is a host trait. This crate never opens a database.
- Embeddings are supplied per chunk, or through an [`Embedder`] the host
  implements. This crate does not load a model.
- Full-text and vector search run in the host. Pass their ranked hits to
  [`reciprocal_rank_fusion`] and a ranker trained on the host's own judgements
  to [`finalize_ranking`].
