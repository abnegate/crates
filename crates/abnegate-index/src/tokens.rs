//! Token estimates and overlapping text windows that break on natural
//! boundaries.

use crate::text_chunk::TextChunk;

const CHARACTERS_PER_TOKEN: usize = 4;

const BREAK_SEARCH_WINDOW: usize = 100;

/// Estimated token count of `text`: its byte length divided by four, rounded
/// up.
pub fn estimate_tokens(text: &str) -> usize {
    text.len().saturating_add(CHARACTERS_PER_TOKEN - 1) / CHARACTERS_PER_TOKEN
}

/// Split `text` into windows of about `maximum_tokens` tokens that overlap by
/// about `overlap_tokens`, breaking at a paragraph, sentence, line or word
/// boundary when one falls near the end of a window.
///
/// When `maximum_tokens` is 0 or `overlap_tokens` is not smaller than it, the
/// whole text is one chunk, even when it is empty. Otherwise empty text yields
/// no chunks.
pub fn chunk_text(text: &str, maximum_tokens: usize, overlap_tokens: usize) -> Vec<TextChunk> {
    if maximum_tokens == 0 || overlap_tokens >= maximum_tokens {
        return vec![TextChunk {
            index: 0,
            text: text.to_string(),
            start_offset: 0,
            end_offset: text.len(),
        }];
    }

    let mut chunks = Vec::new();
    let characters: Vec<char> = text.chars().collect();
    let total_characters = characters.len();

    if total_characters == 0 {
        return chunks;
    }

    let maximum_characters = maximum_tokens * CHARACTERS_PER_TOKEN;
    let overlap_characters = overlap_tokens * CHARACTERS_PER_TOKEN;

    let mut start = 0;
    let mut chunk_index = 0;

    while start < total_characters {
        let mut end = (start + maximum_characters).min(total_characters);

        if end < total_characters {
            end = find_break_point(&characters, start, end);
        }

        let piece: String = characters[start..end].iter().collect();
        let byte_start = text
            .char_indices()
            .nth(start)
            .map(|(index, _)| index)
            .unwrap_or(0);
        let byte_end = if end >= total_characters {
            text.len()
        } else {
            text.char_indices()
                .nth(end)
                .map(|(index, _)| index)
                .unwrap_or(text.len())
        };

        chunks.push(TextChunk {
            index: chunk_index,
            text: piece,
            start_offset: byte_start,
            end_offset: byte_end,
        });

        chunk_index += 1;

        let advance = if end - start > overlap_characters {
            end - start - overlap_characters
        } else {
            end - start
        };

        start += advance;

        if advance == 0 {
            break;
        }
    }

    chunks
}

fn find_break_point(characters: &[char], start: usize, maximum_end: usize) -> usize {
    let search_start = if maximum_end > start + BREAK_SEARCH_WINDOW {
        maximum_end - BREAK_SEARCH_WINDOW
    } else {
        start
    };

    for index in (search_start..maximum_end).rev() {
        if characters[index] == '\n'
            && index + 1 < characters.len()
            && characters[index + 1] == '\n'
        {
            return index + 2;
        }
    }

    for index in (search_start..maximum_end).rev() {
        if (characters[index] == '.' || characters[index] == '!' || characters[index] == '?')
            && index + 1 < characters.len()
            && characters[index + 1].is_whitespace()
        {
            return index + 1;
        }
    }

    for index in (search_start..maximum_end).rev() {
        if characters[index] == '\n' {
            return index + 1;
        }
    }

    for index in (search_start..maximum_end).rev() {
        if characters[index].is_whitespace() {
            return index + 1;
        }
    }

    maximum_end
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_text_is_zero_tokens() {
        assert_eq!(estimate_tokens(""), 0);
    }

    #[test]
    fn a_short_word_is_one_or_two_tokens() {
        let tokens = estimate_tokens("hello");
        assert!((1..=2).contains(&tokens));
    }

    #[test]
    fn a_hundred_characters_are_twenty_five_tokens() {
        let text = "a".repeat(100);
        let tokens = estimate_tokens(&text);
        assert_eq!(tokens, 25);
    }

    #[test]
    fn empty_text_has_no_chunks() {
        let chunks = chunk_text("", 100, 10);
        assert!(chunks.is_empty());
    }

    #[test]
    fn small_text_is_one_chunk_spanning_all_of_it() {
        let text = "Hello world";
        let chunks = chunk_text(text, 100, 10);

        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].text, text);
        assert_eq!(chunks[0].start_offset, 0);
        assert_eq!(chunks[0].end_offset, text.len());
    }

    #[test]
    fn long_text_splits_into_overlapping_chunks_that_cover_it() {
        let text = "This is sentence one. This is sentence two. This is sentence three. This is sentence four.";
        let chunks = chunk_text(text, 10, 2);

        assert!(chunks.len() > 1);
        assert_eq!(chunks[0].start_offset, 0);

        let total_covered: usize = chunks.iter().map(|chunk| chunk.text.len()).sum();
        assert!(total_covered >= text.len());
    }

    #[test]
    fn chunks_break_at_sentence_boundaries() {
        let text = "First sentence. Second sentence. Third sentence.";
        let chunks = chunk_text(text, 8, 1);

        for chunk in &chunks {
            let text = &chunk.text;
            if !text.is_empty() && chunk.end_offset < 48 {
                assert!(
                    text.ends_with(". ") || text.ends_with('.') || text.ends_with(' '),
                    "Chunk should end at boundary: '{}'",
                    text
                );
            }
        }
    }
}
