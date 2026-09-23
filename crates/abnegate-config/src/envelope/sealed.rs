use abnegate_secret::SecretValue;
use toml::Value;

use crate::envelope::location::Location;

/// A value that arrived as an `ENC[v1:...]` envelope.
///
/// It is remembered by where it sat, by what the application was handed there
/// (the plaintext when the loader had a key, the envelope itself when it did
/// not), by how many locations its key path reached through any array element,
/// by how many of those held the same content, and by how many strings in the
/// whole document did.
#[derive(Debug)]
pub(crate) struct Sealed {
    location: Location,
    received: SecretValue,
    reach: usize,
    copies: usize,
    document_copies: usize,
}

impl Sealed {
    /// Remember `received` at `location` in `document` as it was loaded.
    pub(crate) fn new(location: Location, received: SecretValue, document: &Value) -> Self {
        let candidates = location.expand(document);
        let unmeasured = Self {
            location,
            received,
            reach: candidates.len(),
            copies: 0,
            document_copies: 0,
        };

        Self {
            copies: unmeasured.holding(document, &candidates).len(),
            document_copies: unmeasured.copies_in(document),
            ..unmeasured
        }
    }

    pub(crate) fn location(&self) -> &Location {
        &self.location
    }

    /// Whether `text` is what the application was handed for this value,
    /// compared in constant time.
    ///
    /// An empty value matches nothing: every empty string, a defaulted field
    /// among them, would otherwise pass for it.
    pub(crate) fn matches(&self, text: &str) -> bool {
        !self.received.is_empty() && SecretValue::new(text) == self.received
    }

    /// Every location on this value's key path that has to be sealed so it is
    /// never written back in the clear.
    ///
    /// The value is looked for by content through any element of each array on
    /// the way, so it is followed when elements are added or removed. When
    /// fewer locations hold it than did on load, one of them was edited or
    /// removed, and every location on the key path is sealed instead, as it
    /// always is for an empty value.
    pub(crate) fn targets(&self, document: &Value) -> Vec<Location> {
        let candidates = self.location.expand(document);
        let holding = self.holding(document, &candidates);

        if self.received.is_empty() || holding.len() < self.copies {
            candidates
        } else {
            holding
        }
    }

    /// Whether this value may have moved to a new key and been edited on the
    /// way: its key path reaches fewer locations than it did on load and fewer
    /// strings anywhere in `document` hold its content than did then.
    ///
    /// A copy elsewhere is not enough, since it may be a second field that
    /// shared the secret. An empty value cannot be followed by content at all,
    /// so it is lost as soon as its key path shrinks.
    pub(crate) fn is_lost(&self, document: &Value) -> bool {
        self.location.expand(document).len() < self.reach
            && (self.received.is_empty() || self.copies_in(document) < self.document_copies)
    }

    fn holding(&self, document: &Value, candidates: &[Location]) -> Vec<Location> {
        candidates
            .iter()
            .filter(|candidate| {
                candidate
                    .resolve(document)
                    .and_then(Value::as_str)
                    .is_some_and(|text| self.matches(text))
            })
            .cloned()
            .collect()
    }

    fn copies_in(&self, value: &Value) -> usize {
        match value {
            Value::String(text) => usize::from(self.matches(text)),
            Value::Table(table) => table.values().map(|child| self.copies_in(child)).sum(),
            Value::Array(array) => array.iter().map(|child| self.copies_in(child)).sum(),
            _ => 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::envelope::segment::Segment;

    use super::*;

    fn key(name: &str) -> Segment {
        Segment::Key(name.to_string())
    }

    fn hosts(index: usize) -> Location {
        Location::from(vec![key("hosts"), Segment::Index(index)])
    }

    fn password() -> Location {
        Location::from(vec![key("password")])
    }

    fn document(content: &str) -> Value {
        toml::from_str(content).unwrap()
    }

    fn loaded(location: Location, content: &str) -> Sealed {
        Sealed::new(location, SecretValue::new("hunter2"), &document(content))
    }

    fn empty(location: Location, content: &str) -> Sealed {
        Sealed::new(location, SecretValue::new(""), &document(content))
    }

    fn targets(sealed: &Sealed, content: &str) -> Vec<String> {
        sealed
            .targets(&document(content))
            .iter()
            .map(Location::to_string)
            .collect()
    }

    #[test]
    fn a_table_value_is_sealed_where_it_was_whatever_it_now_holds() {
        let sealed = loaded(password(), "password = \"hunter2\"");

        assert_eq!(targets(&sealed, "password = \"edited\""), ["password"]);
    }

    #[test]
    fn a_table_value_whose_key_is_gone_has_nowhere_to_be_sealed() {
        let sealed = loaded(password(), "password = \"hunter2\"");

        assert!(targets(&sealed, "model = \"gpt-4o\"").is_empty());
    }

    #[test]
    fn an_array_value_that_stayed_put_is_sealed_alone() {
        let sealed = loaded(hosts(1), "hosts = [\"one\", \"hunter2\"]");

        assert_eq!(
            targets(&sealed, "hosts = [\"one\", \"hunter2\"]"),
            ["hosts[1]"]
        );
    }

    #[test]
    fn an_array_value_is_followed_when_an_earlier_element_is_removed() {
        let sealed = loaded(hosts(1), "hosts = [\"one\", \"hunter2\"]");

        assert_eq!(targets(&sealed, "hosts = [\"hunter2\"]"), ["hosts[0]"]);
    }

    #[test]
    fn an_array_value_is_followed_when_an_element_is_inserted_before_it() {
        let sealed = loaded(hosts(1), "hosts = [\"one\", \"hunter2\"]");

        assert_eq!(
            targets(&sealed, "hosts = [\"zero\", \"one\", \"hunter2\"]"),
            ["hosts[2]"]
        );
    }

    #[test]
    fn every_copy_of_an_array_value_is_sealed() {
        let sealed = loaded(hosts(0), "hosts = [\"hunter2\", \"one\"]");

        assert_eq!(
            targets(&sealed, "hosts = [\"hunter2\", \"one\", \"hunter2\"]"),
            ["hosts[0]", "hosts[2]"]
        );
    }

    #[test]
    fn an_array_value_that_cannot_be_found_seals_its_whole_key_path() {
        let sealed = loaded(hosts(1), "hosts = [\"one\", \"hunter2\"]");

        assert_eq!(
            targets(&sealed, "hosts = [\"edited\", \"one\"]"),
            ["hosts[0]", "hosts[1]"]
        );
    }

    #[test]
    fn rotating_one_of_two_copies_seals_the_whole_key_path() {
        let sealed = loaded(hosts(0), "hosts = [\"hunter2\", \"hunter2\"]");

        assert_eq!(
            targets(&sealed, "hosts = [\"hunter2\", \"rotated\"]"),
            ["hosts[0]", "hosts[1]"]
        );
    }

    #[test]
    fn a_plain_copy_on_the_key_path_counts_towards_the_copies() {
        let sealed = loaded(hosts(1), "hosts = [\"hunter2\", \"hunter2\"]");

        assert_eq!(
            targets(&sealed, "hosts = [\"hunter2\", \"rotated\"]"),
            ["hosts[0]", "hosts[1]"]
        );
    }

    #[test]
    fn a_value_whose_key_and_content_are_both_gone_is_lost() {
        let sealed = loaded(password(), "password = \"hunter2\"");

        assert!(sealed.is_lost(&document("token = \"edited\"")));
    }

    #[test]
    fn a_value_whose_content_moved_to_another_key_is_not_lost() {
        let sealed = loaded(password(), "password = \"hunter2\"");

        assert!(!sealed.is_lost(&document("[profiles.work]\ntoken = \"hunter2\"")));
    }

    #[test]
    fn a_value_whose_key_is_gone_is_lost_when_fewer_copies_remain() {
        let sealed = loaded(password(), "password = \"hunter2\"\nbackup = \"hunter2\"");

        assert!(sealed.is_lost(&document("token = \"edited\"\nbackup = \"hunter2\"")));
    }

    #[test]
    fn a_value_whose_key_is_gone_is_not_lost_while_every_copy_remains() {
        let sealed = loaded(password(), "password = \"hunter2\"\nbackup = \"hunter2\"");

        assert!(!sealed.is_lost(&document("token = \"hunter2\"\nbackup = \"hunter2\"")));
    }

    #[test]
    fn an_empty_value_whose_key_is_gone_is_lost_whatever_else_is_empty() {
        let sealed = empty(password(), "password = \"\"\nproxy = \"\"");

        assert!(sealed.is_lost(&document("token = \"\"\nproxy = \"\"")));
    }

    #[test]
    fn an_empty_value_edited_where_it_sits_is_not_lost() {
        let sealed = empty(password(), "password = \"\"");

        assert!(!sealed.is_lost(&document("password = \"edited\"")));
    }

    #[test]
    fn an_empty_array_value_seals_its_whole_key_path() {
        let sealed = empty(hosts(0), "hosts = [\"\"]");

        assert_eq!(
            targets(&sealed, "hosts = [\"edited\", \"\"]"),
            ["hosts[0]", "hosts[1]"]
        );
    }

    #[test]
    fn a_value_edited_where_it_sits_is_not_lost() {
        let sealed = loaded(password(), "password = \"hunter2\"");

        assert!(!sealed.is_lost(&document("password = \"edited\"")));
    }

    #[test]
    fn an_array_value_removed_with_its_element_is_lost() {
        let sealed = loaded(hosts(1), "hosts = [\"one\", \"hunter2\"]");

        assert!(sealed.is_lost(&document("hosts = [\"one\"]")));
    }

    #[test]
    fn an_array_value_edited_in_place_is_not_lost() {
        let sealed = loaded(hosts(1), "hosts = [\"one\", \"hunter2\"]");

        assert!(!sealed.is_lost(&document("hosts = [\"one\", \"edited\"]")));
    }

    #[test]
    fn content_is_matched_whole() {
        let sealed = loaded(password(), "password = \"hunter2\"");

        assert!(sealed.matches("hunter2"));
        assert!(!sealed.matches("hunter"));
        assert!(!sealed.matches("hunter22"));
    }

    #[test]
    fn an_empty_value_matches_nothing() {
        let sealed = empty(password(), "password = \"\"");

        assert!(!sealed.matches(""));
    }
}
