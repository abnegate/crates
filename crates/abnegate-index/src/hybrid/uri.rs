//! Matching a file's name against the identifiers a question names.

/// Whether the file stem of `uri` is one of `identifiers`, or contains a part
/// of four or more characters of one, split on `_`, `-` and `/`.
///
/// Stems are lowercased and must be at least four characters; the URI's
/// `@ref` and every extension are dropped first.
pub fn uri_stem_matches_identifier(uri: &str, identifiers: &[String]) -> bool {
    let Some(stem) = uri_file_stem(uri) else {
        return false;
    };
    identifiers.iter().any(|identifier| {
        let lowercase_identifier = identifier.to_ascii_lowercase();
        stem == lowercase_identifier
            || lowercase_identifier
                .split(['_', '-', '/'])
                .any(|part| part.len() >= 4 && (stem == part || stem.contains(part)))
    })
}

pub(super) fn uri_file_stem(uri: &str) -> Option<String> {
    let name = uri
        .rsplit('/')
        .next()?
        .split('@')
        .next()?
        .split('.')
        .next()?
        .to_ascii_lowercase();
    (name.len() >= 4).then_some(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stem_matches_a_whole_identifier_or_a_long_part() {
        let uri = "github://acme/widgets/src/inventory/warehouse.rs@main";
        assert!(uri_stem_matches_identifier(uri, &["Warehouse".into()]));
        assert!(uri_stem_matches_identifier(uri, &["warehouse_bins".into()]));
        assert!(!uri_stem_matches_identifier(uri, &["reserve_stock".into()]));
    }

    #[test]
    fn short_stems_never_match() {
        assert!(!uri_stem_matches_identifier("src/mod.rs", &["mod".into()]));
        assert_eq!(uri_file_stem("src/lib.rs"), None);
        assert_eq!(uri_file_stem("src/Main.test.ts@main"), Some("main".into()));
    }
}
