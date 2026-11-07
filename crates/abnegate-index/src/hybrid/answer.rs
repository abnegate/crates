//! Keeping the chunks that can answer a question ahead of navigation chunks.

use crate::rank::identifier_role;
use crate::rank::is_fixture_uri;
use crate::rank::is_outline_chunk;

/// Keep up to `limit` of `items`, answer chunks first, dropping fixtures.
///
/// `parts` yields an item's `(text, uri)`. Outlines that define none of
/// `identifiers` are navigation: they fill only the slots left once every
/// other item has been kept, in their original order. The first other item is
/// kept even when `limit` is 0.
pub fn keep_answer_chunks<T>(
    items: Vec<T>,
    limit: usize,
    identifiers: &[String],
    parts: impl Fn(&T) -> (&str, &str),
) -> Vec<T> {
    let mut body = Vec::new();
    let mut navigation = Vec::new();
    for item in items {
        let (text, uri) = parts(&item);
        if is_fixture_uri(uri) {
            continue;
        }
        let outline = is_outline_chunk(text);
        let defined = identifier_role(text, uri, identifiers).is_definition();
        if outline && !defined {
            navigation.push(item);
        } else {
            body.push(item);
            if body.len() >= limit {
                return body;
            }
        }
    }
    let remaining = limit.saturating_sub(body.len());
    body.extend(navigation.into_iter().take(remaining));
    body
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hybrid::candidate::Candidate;

    fn parts(candidate: &Candidate<u32>) -> (&str, &str) {
        (candidate.chunk_text.as_str(), candidate.item_uri.as_str())
    }

    #[test]
    fn keep_answer_chunks_drops_headers_first() {
        let header = Candidate::new(
            1,
            "path: x kind: file_header\n\nidentifiers: should_skip_blob",
            "github://acme/widgets/content/mod.rs",
            "mod.rs",
            0.9,
        );
        let body = Candidate::new(
            2,
            "pub fn should_skip_blob(&self) -> bool { true }",
            "github://acme/widgets/content/mod.rs",
            "mod.rs",
            0.4,
        );
        let kept = keep_answer_chunks(vec![header, body], 1, &["should_skip_blob".into()], parts);
        assert_eq!(kept.len(), 1);
        assert!(kept[0].chunk_text.contains("pub fn should_skip_blob"));
    }

    #[test]
    fn a_zero_limit_still_keeps_the_first_answer_chunk() {
        let header = Candidate::new(
            1,
            "path: x kind: file_header\n\nidentifiers: should_skip_blob",
            "github://acme/widgets/content/mod.rs",
            "mod.rs",
            0.9,
        );
        let body = Candidate::new(
            2,
            "pub fn should_skip_blob(&self) -> bool { true }",
            "github://acme/widgets/content/mod.rs",
            "mod.rs",
            0.4,
        );
        let kept = keep_answer_chunks(vec![header, body], 0, &["should_skip_blob".into()], parts);
        let ids: Vec<u32> = kept.iter().map(|candidate| candidate.id).collect();
        assert_eq!(ids, [2]);
    }

    #[test]
    fn navigation_fills_leftover_slots_and_fixtures_are_dropped() {
        let header = Candidate::new(
            1,
            "path: x kind: file_header\n\nidentifiers: should_skip_blob",
            "github://acme/widgets/content/mod.rs",
            "mod.rs",
            0.9,
        );
        let fixture = Candidate::new(
            2,
            "should_skip_blob",
            "github://acme/widgets/evals/retrieval.json@main",
            "retrieval.json",
            0.8,
        );
        let body = Candidate::new(
            3,
            "pub fn should_skip_blob(&self) -> bool { true }",
            "github://acme/widgets/content/mod.rs",
            "mod.rs",
            0.4,
        );
        let kept = keep_answer_chunks(
            vec![header, fixture, body],
            5,
            &["should_skip_blob".into()],
            parts,
        );
        let ids: Vec<u32> = kept.iter().map(|candidate| candidate.id).collect();
        assert_eq!(ids, [3, 1]);
    }
}
