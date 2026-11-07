//! Which of two chunks from one file better represents it for a question.

use std::cmp::Ordering;

use crate::hybrid::coverage::conjunction_identifier_hit;
use crate::hybrid::coverage::defined_leaf_symbol;
use crate::hybrid::coverage::defined_symbol_prefix;
use crate::hybrid::coverage::exclusive_question_hits;
use crate::hybrid::coverage::filename_stem_hits;
use crate::hybrid::coverage::question_coverage;
use crate::hybrid::file::file_row_tier;
use crate::hybrid::question::Question;
use crate::hybrid::result::HybridSearchResult;
use crate::rank::answers_as_definition;
use crate::rank::lexical_cross_score;
use crate::rank::role_strength;

pub(super) fn compare_file_chunks<Id>(
    question: &Question<'_>,
    left: &HybridSearchResult<Id>,
    right: &HybridSearchResult<Id>,
) -> Ordering {
    let query = question.text;
    file_row_tier(&left.item_uri, &left.chunk_text)
        .cmp(&file_row_tier(&right.item_uri, &right.chunk_text))
        .then_with(|| {
            answers_as_definition(query, &left.item_uri, &left.chunk_text).cmp(
                &answers_as_definition(query, &right.item_uri, &right.chunk_text),
            )
        })
        .then_with(|| {
            let same_symbol = defined_leaf_symbol(&left.chunk_text)
                .zip(defined_leaf_symbol(&right.chunk_text))
                .is_some_and(|(left_symbol, right_symbol)| left_symbol == right_symbol);
            if same_symbol {
                filename_stem_hits(&left.item_uri, &left.chunk_text)
                    .cmp(&filename_stem_hits(&right.item_uri, &right.chunk_text))
            } else {
                Ordering::Equal
            }
        })
        .then_with(|| {
            conjunction_identifier_hit(&left.chunk_text, &question.identifiers).cmp(
                &conjunction_identifier_hit(&right.chunk_text, &question.identifiers),
            )
        })
        .then_with(|| {
            defined_symbol_prefix(question, &left.chunk_text)
                .cmp(&defined_symbol_prefix(question, &right.chunk_text))
        })
        .then_with(|| {
            exclusive_question_hits(question, &left.chunk_text, &right.chunk_text).cmp(
                &exclusive_question_hits(question, &right.chunk_text, &left.chunk_text),
            )
        })
        .then_with(|| {
            question_coverage(question, &left.chunk_text)
                .cmp(&question_coverage(question, &right.chunk_text))
        })
        .then_with(|| {
            role_strength(&left.chunk_text, &left.item_uri, &question.identifiers).cmp(
                &role_strength(&right.chunk_text, &right.item_uri, &question.identifiers),
            )
        })
        .then_with(|| {
            let left_lexical =
                lexical_cross_score(query, &left.item_uri, &left.item_title, &left.chunk_text);
            let right_lexical =
                lexical_cross_score(query, &right.item_uri, &right.item_title, &right.chunk_text);
            left_lexical
                .partial_cmp(&right_lexical)
                .unwrap_or(Ordering::Equal)
        })
        .then_with(|| {
            left.score
                .partial_cmp(&right.score)
                .unwrap_or(Ordering::Equal)
        })
}
