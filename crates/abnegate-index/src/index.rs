//! Chunk and persist a tree or a list of blobs.

use std::path::Path;

use crate::cancel::Cancel;
use crate::chunk::SMALLEST;
use crate::chunk::chunks;
use crate::embedder::Embedder;
use crate::error::Result;
use crate::generated::generated;
use crate::hash::file_hash;
use crate::language::language_of;
use crate::relative::relative_to;
use crate::relative::root_key;
use crate::stats::Stats;
use crate::store::Store;
use crate::walk::Walk;

/// Windows embedded together in one [`Embedder::embed`] call.
pub const EMBED_BATCH: usize = 32;

/// Chunk and persist every source file under `root`.
///
/// When `embedder` is `None`, text still lands in the store so later keyword
/// search works. Unchanged files are skipped by content hash.
pub fn index_tree(
    store: &dyn Store,
    embedder: Option<&dyn Embedder>,
    root: &Path,
    cancel: &dyn Cancel,
) -> Result<Stats> {
    index_walk(store, embedder, root, &Walk::new(), cancel)
}

/// [`index_tree`] using `walk` instead of [`Walk::new`].
pub fn index_walk(
    store: &dyn Store,
    embedder: Option<&dyn Embedder>,
    root: &Path,
    walk: &Walk,
    cancel: &dyn Cancel,
) -> Result<Stats> {
    let key = root_key(root);
    let mut stats = Stats::new(key.clone());
    if !root.is_dir() {
        return Ok(stats);
    }
    let mut pending = Vec::new();
    for path in walk.collect(root) {
        cancel.check()?;
        let relative = relative_to(root, &path);
        let Ok(blob) = std::fs::read_to_string(&path) else {
            continue;
        };
        if generated(&relative, &blob) {
            continue;
        }
        if blob.trim().len() < SMALLEST {
            continue;
        }
        let hash = file_hash(&blob);
        if store.file_current(&key, &relative, &hash)? {
            stats.skipped += 1;
            continue;
        }
        let pieces = chunks(&blob);
        if pieces.is_empty() {
            continue;
        }
        let language = language_of(&relative);
        pending.push(FileWork {
            relative,
            language,
            hash,
            pieces,
        });
    }
    persist(store, embedder, &key, pending, &mut stats, cancel)?;
    Ok(stats)
}

/// Index caller-supplied blobs (crash stacks, syscall descriptions, diffs).
pub fn index_texts(
    store: &dyn Store,
    embedder: Option<&dyn Embedder>,
    root: &str,
    items: &[(String, String)],
    cancel: &dyn Cancel,
) -> Result<Stats> {
    let key = root.replace('\\', "/");
    let mut stats = Stats::new(key.clone());
    let mut pending = Vec::new();
    for (relative, blob) in items {
        cancel.check()?;
        if generated(relative, blob) {
            continue;
        }
        if blob.trim().len() < SMALLEST {
            continue;
        }
        let hash = file_hash(blob);
        if store.file_current(&key, relative, &hash)? {
            stats.skipped += 1;
            continue;
        }
        let pieces = chunks(blob);
        if pieces.is_empty() {
            continue;
        }
        pending.push(FileWork {
            relative: relative.clone(),
            language: language_of(relative),
            hash,
            pieces,
        });
    }
    persist(store, embedder, &key, pending, &mut stats, cancel)?;
    Ok(stats)
}

struct FileWork {
    relative: String,
    language: String,
    hash: String,
    pieces: Vec<String>,
}

fn persist(
    store: &dyn Store,
    embedder: Option<&dyn Embedder>,
    root: &str,
    pending: Vec<FileWork>,
    stats: &mut Stats,
    cancel: &dyn Cancel,
) -> Result<()> {
    stats.files = pending.len();
    if pending.is_empty() {
        return Ok(());
    }
    let texts: Vec<&str> = pending
        .iter()
        .flat_map(|work| work.pieces.iter().map(String::as_str))
        .collect();
    stats.chunks = texts.len();
    let mut vectors: Vec<Option<Vec<f32>>> = vec![None; texts.len()];
    if let Some(embedder) = embedder {
        let mut encoded = Vec::new();
        for batch in texts.chunks(EMBED_BATCH) {
            cancel.check()?;
            match embedder.embed(batch) {
                Ok(batch_vectors) if batch_vectors.len() == batch.len() => {
                    encoded.extend(batch_vectors);
                }
                _ => {
                    encoded.clear();
                    break;
                }
            }
        }
        if encoded.len() == texts.len() {
            stats.embedded = encoded.len();
            for (slot, vector) in vectors.iter_mut().zip(encoded) {
                *slot = Some(vector);
            }
        }
    }
    let mut cursor = 0usize;
    for work in &pending {
        cancel.check()?;
        let count = work.pieces.len();
        let slice: Vec<(String, Option<Vec<f32>>)> = work
            .pieces
            .iter()
            .zip(vectors[cursor..cursor + count].iter())
            .map(|(text, vector)| (text.clone(), vector.clone()))
            .collect();
        cursor += count;
        store.replace_file(root, &work.relative, &work.language, &work.hash, &slice)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::Memory;
    use crate::proceed::Proceed;
    use crate::scripted::Scripted;
    use crate::similarity::SIMILARITY_FLOOR;
    use crate::store::Store;
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;

    struct CountingCancel {
        remaining: AtomicUsize,
    }

    impl Cancel for CountingCancel {
        fn check(&self) -> Result<()> {
            let left = self.remaining.fetch_sub(1, Ordering::SeqCst);
            if left == 0 {
                return Err(crate::error::Error::cancel("stopped"));
            }
            Ok(())
        }
    }

    #[test]
    fn index_tree_persists_and_cosine_ranks() {
        let root = tempfile::tempdir().unwrap();
        let api = root.path().join("lib").join("api");
        std::fs::create_dir_all(&api).unwrap();
        std::fs::write(
            api.join("bulk_imports.rb"),
            "class API::BulkImports < Grape::API\n  get ':id/entities' do\n    present entities\n  end\nend\n",
        )
        .unwrap();
        std::fs::write(
            api.join("version.rb"),
            "class API::Version < Grape::API\n  get '/version' do\n    { version: '1' }\n  end\nend\n",
        )
        .unwrap();
        let embedder = Arc::new(Scripted::new(4));
        embedder.insert_containing("get ':id/entities'", vec![1.0, 0.0, 0.0, 0.0]);
        embedder.insert_containing("get '/version'", vec![0.0, 1.0, 0.0, 0.0]);
        let store = Memory::new().with_embedder(embedder.clone());
        let stats = index_tree(&store, Some(embedder.as_ref()), root.path(), &Proceed).unwrap();
        assert_eq!(stats.files, 2);
        assert!(stats.chunks >= 2);
        assert_eq!(stats.embedded, stats.chunks);
        let hits = store
            .search("get ':id/entities' present entities", None, 4)
            .unwrap();
        assert!(
            hits.iter().any(
                |hit| hit.relative.contains("bulk_imports.rb") && hit.score >= SIMILARITY_FLOOR
            ),
            "{hits:?}"
        );
        let again = index_tree(&store, Some(embedder.as_ref()), root.path(), &Proceed).unwrap();
        assert_eq!(again.skipped, 2);
        assert_eq!(again.files, 0);
    }

    #[test]
    fn index_texts_skips_generated_blobs() {
        let store = Memory::new();
        let stats = index_texts(
            &store,
            None,
            "/src",
            &[
                (
                    "types.pb.rs".into(),
                    "pub struct Foo {}\nimpl Foo {}\n".into(),
                ),
                (
                    "api.rs".into(),
                    "fn search_entities() {\n    todo!()\n}\n".into(),
                ),
            ],
            &Proceed,
        )
        .unwrap();
        assert_eq!(stats.files, 1);
        let hits = store.search("search_entities", None, 4).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].relative, "api.rs");
    }

    #[test]
    fn index_tree_stops_when_cancel_fires() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("a.rs"), "fn alpha() { todo!() }\n").unwrap();
        let store = Memory::new();
        let cancel = CountingCancel {
            remaining: AtomicUsize::new(1),
        };
        let error = index_tree(&store, None, root.path(), &cancel).unwrap_err();
        assert!(error.to_string().contains("stopped"));
    }

    #[test]
    fn missing_root_is_an_empty_pass() {
        let store = Memory::new();
        let stats = index_tree(&store, None, Path::new("/no-such-index-root"), &Proceed).unwrap();
        assert_eq!(stats.files, 0);
        assert!(!stats.root.is_empty());
    }
}
