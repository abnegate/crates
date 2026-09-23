use std::fmt;

use toml::Value;

use crate::envelope::segment::Segment;

/// Where a value sits in a TOML document: the keys and array indices that lead
/// to it from the root.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct Location {
    segments: Vec<Segment>,
}

impl Location {
    pub(crate) fn push(&mut self, segment: Segment) {
        self.segments.push(segment);
    }

    pub(crate) fn pop(&mut self) {
        self.segments.pop();
    }

    pub(crate) fn resolve<'document>(
        &self,
        document: &'document Value,
    ) -> Option<&'document Value> {
        let mut current = document;

        for segment in &self.segments {
            current = match segment {
                Segment::Key(name) => current.as_table()?.get(name)?,
                Segment::Index(index) => current.as_array()?.get(*index)?,
            };
        }

        Some(current)
    }

    pub(crate) fn resolve_mut<'document>(
        &self,
        document: &'document mut Value,
    ) -> Option<&'document mut Value> {
        let mut current = document;

        for segment in &self.segments {
            current = match segment {
                Segment::Key(name) => current.as_table_mut()?.get_mut(name)?,
                Segment::Index(index) => current.as_array_mut()?.get_mut(*index)?,
            };
        }

        Some(current)
    }

    /// Every location in `document` reached by the same keys through any
    /// element of each array on the way.
    pub(crate) fn expand(&self, document: &Value) -> Vec<Location> {
        let mut found = Vec::new();
        expand(
            &self.segments,
            document,
            &mut Location::default(),
            &mut found,
        );
        found
    }
}

impl From<Vec<Segment>> for Location {
    fn from(segments: Vec<Segment>) -> Self {
        Self { segments }
    }
}

impl fmt::Display for Location {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (position, segment) in self.segments.iter().enumerate() {
            match segment {
                Segment::Key(name) if position == 0 => formatter.write_str(name)?,
                Segment::Key(name) => write!(formatter, ".{name}")?,
                Segment::Index(index) => write!(formatter, "[{index}]")?,
            }
        }

        Ok(())
    }
}

fn expand(segments: &[Segment], value: &Value, prefix: &mut Location, found: &mut Vec<Location>) {
    let Some((first, rest)) = segments.split_first() else {
        found.push(prefix.clone());
        return;
    };

    match first {
        Segment::Key(name) => {
            if let Some(child) = value.as_table().and_then(|table| table.get(name)) {
                prefix.push(Segment::Key(name.clone()));
                expand(rest, child, prefix, found);
                prefix.pop();
            }
        }
        Segment::Index(_) => {
            for (index, child) in value.as_array().into_iter().flatten().enumerate() {
                prefix.push(Segment::Index(index));
                expand(rest, child, prefix, found);
                prefix.pop();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(name: &str) -> Segment {
        Segment::Key(name.to_string())
    }

    fn document() -> Value {
        toml::from_str(
            r#"
            model = "gpt-4o"
            hosts = ["one", "two", "three"]

            [[servers]]
            name = "a"
            password = "first"

            [[servers]]
            name = "b"

            [[servers]]
            name = "c"
            password = "third"
            "#,
        )
        .unwrap()
    }

    #[test]
    fn a_location_reads_as_a_dotted_path() {
        assert_eq!(Location::default().to_string(), "");
        assert_eq!(
            Location::from(vec![key("database"), key("password")]).to_string(),
            "database.password"
        );
        assert_eq!(
            Location::from(vec![key("hosts"), Segment::Index(2)]).to_string(),
            "hosts[2]"
        );
        assert_eq!(
            Location::from(vec![key("servers"), Segment::Index(0), key("password")]).to_string(),
            "servers[0].password"
        );
    }

    #[test]
    fn a_location_resolves_to_its_value() {
        let document = document();

        assert_eq!(
            Location::from(vec![key("hosts"), Segment::Index(1)])
                .resolve(&document)
                .and_then(Value::as_str),
            Some("two")
        );
        assert!(
            Location::from(vec![key("hosts"), Segment::Index(9)])
                .resolve(&document)
                .is_none()
        );
        assert!(
            Location::from(vec![key("model"), key("nested")])
                .resolve(&document)
                .is_none()
        );
    }

    #[test]
    fn expanding_visits_every_element_of_each_array() {
        let document = document();

        let expanded: Vec<String> = Location::from(vec![key("hosts"), Segment::Index(7)])
            .expand(&document)
            .iter()
            .map(Location::to_string)
            .collect();

        assert_eq!(expanded, ["hosts[0]", "hosts[1]", "hosts[2]"]);
    }

    #[test]
    fn expanding_skips_elements_without_the_key() {
        let document = document();

        let expanded: Vec<String> =
            Location::from(vec![key("servers"), Segment::Index(1), key("password")])
                .expand(&document)
                .iter()
                .map(Location::to_string)
                .collect();

        assert_eq!(expanded, ["servers[0].password", "servers[2].password"]);
    }

    #[test]
    fn expanding_a_path_without_an_index_finds_at_most_itself() {
        let document = document();

        assert_eq!(
            Location::from(vec![key("model")]).expand(&document),
            [Location::from(vec![key("model")])]
        );
        assert!(
            Location::from(vec![key("absent")])
                .expand(&document)
                .is_empty()
        );
    }
}
