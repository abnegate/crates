//! In-memory chunk store.

use std::sync::Arc;
use std::sync::Mutex;

use crate::embedder::Embedder;
use crate::error::Error;
use crate::error::Result;
use crate::hit::Hit;
use crate::method::Method;
use crate::similarity::cosine_similarity;
use crate::store::Store;

struct Row {
    id: i64,
    root: String,
    relative: String,
    language: String,
    hash: String,
    text: String,
    vector: Option<Vec<f32>>,
}

/// In-memory chunk store. A host that wants durability implements [`Store`].
#[derive(Default)]
pub struct Memory {
    rows: Mutex<Vec<Row>>,
    next_id: Mutex<i64>,
    embedder: Option<Arc<dyn Embedder>>,
}

impl Memory {
    /// An empty store with no embedder.
    pub fn new() -> Self {
        Self {
            rows: Mutex::new(Vec::new()),
            next_id: Mutex::new(1),
            embedder: None,
        }
    }

    /// This store, embedding queries with `embedder` during [`Store::search`].
    pub fn with_embedder(mut self, embedder: Arc<dyn Embedder>) -> Self {
        self.embedder = Some(embedder);
        self
    }

    fn rows(&self) -> Result<std::sync::MutexGuard<'_, Vec<Row>>> {
        self.rows
            .lock()
            .map_err(|error| Error::store(error.to_string()))
    }

    fn next_id(&self) -> Result<i64> {
        let mut next = self
            .next_id
            .lock()
            .map_err(|error| Error::store(error.to_string()))?;
        let id = *next;
        *next += 1;
        Ok(id)
    }

    fn search_keyword(&self, query: &str, root: Option<&str>, limit: usize) -> Result<Vec<Hit>> {
        let mut scored: Vec<Hit> = self
            .rows()?
            .iter()
            .filter(|row| root.is_none_or(|root| row.root == root))
            .filter_map(|row| {
                let score = keyword_score(query, &row.text);
                (score > 0.0).then(|| {
                    Hit::new(
                        row.id,
                        row.root.clone(),
                        row.relative.clone(),
                        row.language.clone(),
                        row.text.clone(),
                    )
                    .with_score(score)
                    .with_method(Method::Keyword)
                })
            })
            .collect();
        sort_hits(&mut scored);
        scored.truncate(limit);
        Ok(scored)
    }
}

impl Store for Memory {
    fn file_current(&self, root: &str, relative: &str, hash: &str) -> Result<bool> {
        Ok(self
            .rows()?
            .iter()
            .any(|row| row.root == root && row.relative == relative && row.hash == hash))
    }

    fn replace_file(
        &self,
        root: &str,
        relative: &str,
        language: &str,
        hash: &str,
        chunks: &[(String, Option<Vec<f32>>)],
    ) -> Result<()> {
        let mut rows = self.rows()?;
        rows.retain(|row| !(row.root == root && row.relative == relative));
        drop(rows);
        for (text, vector) in chunks {
            let id = self.next_id()?;
            self.rows()?.push(Row {
                id,
                root: root.to_string(),
                relative: relative.to_string(),
                language: language.to_string(),
                hash: hash.to_string(),
                text: text.clone(),
                vector: vector.clone(),
            });
        }
        Ok(())
    }

    fn search(&self, query: &str, root: Option<&str>, limit: usize) -> Result<Vec<Hit>> {
        if query.trim().is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        if let Some(embedder) = &self.embedder {
            match embedder.embed(&[query]) {
                Ok(vectors) if vectors.len() == 1 && !vectors[0].is_empty() => {
                    let hits = self.search_vector(&vectors[0], root, limit)?;
                    if !hits.is_empty() {
                        return Ok(hits);
                    }
                }
                _ => {}
            }
        }
        self.search_keyword(query, root, limit)
    }

    fn search_vector(&self, vector: &[f32], root: Option<&str>, limit: usize) -> Result<Vec<Hit>> {
        if vector.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let mut scored: Vec<Hit> = self
            .rows()?
            .iter()
            .filter(|row| root.is_none_or(|root| row.root == root))
            .filter_map(|row| {
                let stored = row.vector.as_deref()?;
                Some(
                    Hit::new(
                        row.id,
                        row.root.clone(),
                        row.relative.clone(),
                        row.language.clone(),
                        row.text.clone(),
                    )
                    .with_score(cosine_similarity(vector, stored))
                    .with_method(Method::Cosine),
                )
            })
            .collect();
        sort_hits(&mut scored);
        scored.truncate(limit);
        Ok(scored)
    }
}

fn keyword_score(query: &str, text: &str) -> f64 {
    let folded = text.to_ascii_lowercase();
    let tokens: Vec<String> = query
        .split_whitespace()
        .map(str::to_ascii_lowercase)
        .filter(|token| token.len() > 1)
        .collect();
    if tokens.is_empty() {
        return 0.0;
    }
    let hits = tokens
        .iter()
        .filter(|token| folded.contains(token.as_str()))
        .count();
    hits as f64 / tokens.len() as f64
}

fn sort_hits(hits: &mut [Hit]) {
    hits.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyword_search_ranks_the_matching_file() {
        let store = Memory::new();
        store
            .replace_file(
                "/src",
                "bulk.rb",
                "rb",
                "a",
                &[("get ':id/entities'".into(), None)],
            )
            .unwrap();
        store
            .replace_file(
                "/src",
                "version.rb",
                "rb",
                "b",
                &[("get '/version'".into(), None)],
            )
            .unwrap();
        let hits = store.search("entities", Some("/src"), 4).unwrap();
        assert_eq!(hits[0].relative, "bulk.rb");
        assert_eq!(hits[0].method, Method::Keyword);
    }

    #[test]
    fn vector_search_ranks_by_cosine() {
        let store = Memory::new();
        store
            .replace_file(
                "/src",
                "bulk.rb",
                "rb",
                "a",
                &[("entities".into(), Some(vec![1.0, 0.0]))],
            )
            .unwrap();
        store
            .replace_file(
                "/src",
                "version.rb",
                "rb",
                "b",
                &[("version".into(), Some(vec![0.0, 1.0]))],
            )
            .unwrap();
        let hits = store.search_vector(&[1.0, 0.0], None, 2).unwrap();
        assert_eq!(hits[0].relative, "bulk.rb");
        assert!((hits[0].score - 1.0).abs() < 1e-6);
        assert_eq!(hits[0].method, Method::Cosine);
    }

    #[test]
    fn replace_file_deletes_the_previous_chunks() {
        let store = Memory::new();
        store
            .replace_file("/src", "a.rs", "rs", "1", &[("old".into(), None)])
            .unwrap();
        store
            .replace_file("/src", "a.rs", "rs", "2", &[("new".into(), None)])
            .unwrap();
        let hits = store.search("old", None, 4).unwrap();
        assert!(hits.is_empty());
        assert_eq!(store.search("new", None, 4).unwrap()[0].text, "new");
    }
}
