use serde::Deserialize;
use serde::Deserializer;
use std::collections::BTreeMap;
use std::collections::BTreeSet;

/// The packages one section of a manifest requires, by name. PHP encodes an
/// empty object as `[]`, so a Composer manifest with nothing in a section
/// often carries an empty list there, which requires nothing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct Requirements(BTreeSet<String>);

impl Requirements {
    pub(super) fn names(&self) -> impl Iterator<Item = &str> {
        self.0.iter().map(String::as_str)
    }
}

impl<'de> Deserialize<'de> for Requirements {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Section {
            Named(BTreeMap<String, serde_json::Value>),
            Empty([(); 0]),
        }

        Ok(match Option::<Section>::deserialize(deserializer)? {
            Some(Section::Named(named)) => Self(named.into_keys().collect()),
            Some(Section::Empty(_)) | None => Self::default(),
        })
    }
}
