//! The terms of one question, derived once per ranking pass.

use crate::query::bridge_terms;
use crate::query::content_tokens;
use crate::query::path_uri_tokens;
use crate::query::question_phrases;
use crate::query::rewrite_query;

pub(super) struct Question<'a> {
    pub(super) text: &'a str,
    pub(super) identifiers: Vec<String>,
    pub(super) content_tokens: Vec<String>,
    pub(super) bridge_terms: Vec<String>,
    pub(super) phrases: Vec<String>,
    pub(super) stems: Vec<String>,
    pub(super) path_tokens: Vec<String>,
}

impl<'a> Question<'a> {
    pub(super) fn new(text: &'a str) -> Self {
        let content_tokens = content_tokens(text);
        let stems = content_tokens
            .iter()
            .filter_map(|token| question_stem(token))
            .map(str::to_string)
            .collect();
        Self {
            text,
            identifiers: rewrite_query(text).identifiers,
            content_tokens,
            bridge_terms: bridge_terms(text),
            phrases: question_phrases(text),
            stems,
            path_tokens: path_uri_tokens(text),
        }
    }

    pub(super) fn tokens_and_stems(&self) -> impl Iterator<Item = &str> {
        self.content_tokens
            .iter()
            .chain(self.stems.iter())
            .map(String::as_str)
    }
}

fn question_stem(token: &str) -> Option<&'static str> {
    match token {
        "derived" => Some("derive"),
        "authorized" | "authorizing" => Some("authorize"),
        "configured" => Some("configure"),
        "trimmed" => Some("trim"),
        _ => None,
    }
}
