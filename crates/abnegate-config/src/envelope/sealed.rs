use abnegate_secret::SecretValue;
use toml::Value;

use crate::envelope::location::Location;

/// A value that arrived as an `ENC[v1:...]` envelope, remembered by where it
/// sat and by what the application was handed there: the plaintext when the
/// loader had a key, the envelope itself when it did not.
#[derive(Debug)]
pub(crate) struct Sealed {
    location: Location,
    received: SecretValue,
}

impl Sealed {
    pub(crate) fn new(location: Location, received: SecretValue) -> Self {
        Self { location, received }
    }

    /// Every location in `document` that has to be sealed so this value is
    /// never written back in the clear.
    ///
    /// A value under a table key stays where it was. A value in an array can
    /// drift when elements are added or removed, so it is looked for by
    /// content across the same key path, and when it cannot be found (it was
    /// edited, or removed) every string on that key path is sealed instead.
    pub(crate) fn targets(&self, document: &Value) -> Vec<Location> {
        if !self.location.is_indexed() {
            return vec![self.location.clone()];
        }

        let candidates = self.location.expand(document);
        let holding: Vec<Location> = candidates
            .iter()
            .filter(|candidate| self.is_held_at(document, candidate))
            .cloned()
            .collect();

        if holding.is_empty() {
            candidates
        } else {
            holding
        }
    }

    fn is_held_at(&self, document: &Value, location: &Location) -> bool {
        location
            .resolve(document)
            .and_then(Value::as_str)
            .is_some_and(|text| SecretValue::new(text) == self.received)
    }
}

#[cfg(test)]
mod tests {
    use crate::envelope::segment::Segment;

    use super::*;

    fn hosts(index: usize) -> Location {
        Location::from(vec![
            Segment::Key("hosts".to_string()),
            Segment::Index(index),
        ])
    }

    fn document(content: &str) -> Value {
        toml::from_str(content).unwrap()
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
        let sealed = Sealed::new(
            Location::from(vec![Segment::Key("password".to_string())]),
            SecretValue::new("hunter2"),
        );

        assert_eq!(targets(&sealed, "password = \"edited\""), ["password"]);
    }

    #[test]
    fn an_array_value_that_stayed_put_is_sealed_alone() {
        let sealed = Sealed::new(hosts(1), SecretValue::new("hunter2"));

        assert_eq!(
            targets(&sealed, "hosts = [\"one\", \"hunter2\"]"),
            ["hosts[1]"]
        );
    }

    #[test]
    fn an_array_value_is_followed_when_an_earlier_element_is_removed() {
        let sealed = Sealed::new(hosts(1), SecretValue::new("hunter2"));

        assert_eq!(targets(&sealed, "hosts = [\"hunter2\"]"), ["hosts[0]"]);
    }

    #[test]
    fn an_array_value_is_followed_when_an_element_is_inserted_before_it() {
        let sealed = Sealed::new(hosts(1), SecretValue::new("hunter2"));

        assert_eq!(
            targets(&sealed, "hosts = [\"zero\", \"one\", \"hunter2\"]"),
            ["hosts[2]"]
        );
    }

    #[test]
    fn every_copy_of_an_array_value_is_sealed() {
        let sealed = Sealed::new(hosts(0), SecretValue::new("hunter2"));

        assert_eq!(
            targets(&sealed, "hosts = [\"hunter2\", \"one\", \"hunter2\"]"),
            ["hosts[0]", "hosts[2]"]
        );
    }

    #[test]
    fn an_array_value_that_cannot_be_found_seals_its_whole_key_path() {
        let sealed = Sealed::new(hosts(1), SecretValue::new("hunter2"));

        assert_eq!(
            targets(&sealed, "hosts = [\"edited\", \"one\"]"),
            ["hosts[0]", "hosts[1]"]
        );
    }
}
